use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    process::Command,
};

use freedesktop_desktop_entry::{DesktopEntry, Iter, default_paths, get_languages_from_env};
use iced::{
    Background, Border, Color, Element, Length, Theme,
    alignment::Horizontal,
    padding::right,
    widget::{button, column, container, grid, image, row, scrollable, svg, text, text_input},
};

const SEARCH_INPUT_ID: &str = "app-search";
const GRID_COLUMNS: usize = 5;

fn result_id(index: usize) -> String {
    format!("app-result-{index}")
}

const ACCENT: Color = Color::from_rgb(0.40, 0.78, 0.98);
const TEXT: Color = Color::from_rgb(0.94, 0.96, 1.0);
const MUTED: Color = Color::from_rgb(0.66, 0.72, 0.81);
const GLASS: Color = Color::from_rgba(0.035, 0.055, 0.09, 0.50);
const TILE: Color = Color::from_rgba(0.12, 0.16, 0.23, 0.76);
const TILE_HOVER: Color = Color::from_rgba(0.19, 0.26, 0.36, 0.94);
const TILE_FOCUSED: Color = Color::from_rgba(0.16, 0.24, 0.34, 0.96);

#[derive(Debug, Clone)]
struct CardFocusState {
    focused: bool,
}

impl iced::advanced::widget::operation::Focusable for CardFocusState {
    fn is_focused(&self) -> bool {
        self.focused
    }

    fn focus(&mut self) {
        self.focused = true;
    }

    fn unfocus(&mut self) {
        self.focused = false;
    }
}

#[derive(Debug, Clone)]
struct AppEntry {
    name: String,
    search_text: String,
    icon: Option<PathBuf>,
    command: Vec<String>,
    working_directory: Option<PathBuf>,
    terminal: bool,
}

struct FocusableCard<'a, Message: Clone + 'static> {
    content: Element<'a, Message>,
    id: iced::advanced::widget::Id,
}

impl<'a, Message: Clone + 'static> FocusableCard<'a, Message> {
    fn new(content: impl Into<Element<'a, Message>>, id: String) -> Self {
        Self {
            content: content.into(),
            id: iced::advanced::widget::Id::from(id),
        }
    }
}

impl<Message: Clone + 'static> iced::advanced::Widget<Message, Theme, iced::Renderer>
    for FocusableCard<'_, Message>
where
    Message: Clone + 'static,
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        iced::advanced::widget::tree::Tag::of::<CardFocusState>()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::new(CardFocusState { focused: false })
    }

    fn children(&self) -> Vec<iced::advanced::widget::Tree> {
        vec![iced::advanced::widget::Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut iced::advanced::widget::Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> iced::Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        renderer: &iced::Renderer,
        limits: &iced::advanced::layout::Limits,
    ) -> iced::advanced::layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        operation.focusable(
            Some(&self.id),
            layout.bounds(),
            tree.state.downcast_mut::<CardFocusState>(),
        );
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout<'_>,
        cursor: iced::mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn iced::advanced::Clipboard,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &iced::Rectangle,
    ) {
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn draw(
        &self,
        tree: &iced::advanced::widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &Theme,
        style: &iced::advanced::renderer::Style,
        layout: iced::advanced::Layout<'_>,
        cursor: iced::mouse::Cursor,
        viewport: &iced::Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &iced::advanced::widget::Tree,
        layout: iced::advanced::Layout<'_>,
        cursor: iced::mouse::Cursor,
        viewport: &iced::Rectangle,
        renderer: &iced::Renderer,
    ) -> iced::mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &iced::Rectangle,
        translation: iced::Vector,
    ) -> Option<iced::advanced::overlay::Element<'b, Message, Theme, iced::Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, Message: Clone + 'static> From<FocusableCard<'a, Message>>
    for Element<'a, Message, Theme, iced::Renderer>
where
    Message: Clone + 'static,
{
    fn from(card: FocusableCard<'a, Message>) -> Self {
        Self::new(card)
    }
}

#[derive(Debug, Clone)]
enum Message {
    SearchChanged(String),
    Keyboard(iced::keyboard::Event),
    RefreshSearchFocus,
    SearchFocusChanged(bool),
    CloseLauncher,
    WindowUnfocused,
    LaunchFocused,
    Launch(usize),
}

struct Launcher {
    apps: Vec<AppEntry>,
    query: String,
    result_focus: Option<usize>,
    search_focused: bool,
    status: String,
}

impl Launcher {
    fn new() -> Self {
        let apps = load_apps();

        Self {
            apps,
            query: String::new(),
            result_focus: None,
            search_focused: true,
            status: String::new(),
        }
    }

    fn update(&mut self, message: Message) -> iced::Task<Message> {
        match message {
            Message::SearchChanged(query) => {
                self.query = query;
                self.result_focus = None;
                self.search_focused = true;
            }
            Message::RefreshSearchFocus => {
                return iced::widget::operation::is_focused(SEARCH_INPUT_ID)
                    .map(Message::SearchFocusChanged);
            }
            Message::SearchFocusChanged(focused) => self.search_focused = focused,
            Message::CloseLauncher => return close_window(),
            Message::WindowUnfocused => return close_window(),
            Message::Keyboard(iced::keyboard::Event::KeyPressed { key, text, .. }) => {
                if key == iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape) {
                    if self.search_focused {
                        return close_window();
                    }
                    self.query.clear();
                    self.result_focus = None;
                    self.search_focused = true;
                    return iced::widget::operation::focus(SEARCH_INPUT_ID)
                        .chain(iced::widget::operation::move_cursor_to_end(SEARCH_INPUT_ID));
                }

                if let iced::keyboard::Key::Named(named) = key
                    && named == iced::keyboard::key::Named::Enter
                    && let Some(index) = self.result_focus
                    && !self.search_focused
                {
                    return self.launch_index(index);
                }

                if key == iced::keyboard::Key::Named(iced::keyboard::key::Named::Tab)
                    && self.search_focused
                {
                    let result_index = self.result_focus.unwrap_or(0);
                    self.result_focus = Some(result_index);
                    return iced::widget::operation::focus(result_id(result_index));
                }

                let key_step = match key {
                    iced::keyboard::Key::Named(iced::keyboard::key::Named::ArrowDown) => {
                        Some(GRID_COLUMNS as isize)
                    }
                    iced::keyboard::Key::Named(iced::keyboard::key::Named::ArrowUp) => {
                        Some(-(GRID_COLUMNS as isize))
                    }
                    iced::keyboard::Key::Named(iced::keyboard::key::Named::ArrowRight) => Some(1),
                    iced::keyboard::Key::Named(iced::keyboard::key::Named::ArrowLeft) => Some(-1),
                    _ => None,
                };

                if !self.search_focused
                    && self.result_focus.is_some()
                    && matches!(key, iced::keyboard::Key::Character(_))
                    && let Some(typed) = text
                    && !typed.is_empty()
                    && typed.chars().all(|character| !character.is_control())
                {
                    self.query.push_str(&typed);
                    self.search_focused = true;
                    self.result_focus = None;
                    return iced::widget::operation::focus(SEARCH_INPUT_ID)
                        .chain(iced::widget::operation::move_cursor_to_end(SEARCH_INPUT_ID));
                }

                if let Some(step) = key_step {
                    let apps_len = self.filtered_apps().len();
                    if apps_len == 0 {
                        return iced::Task::none();
                    }

                    if self.search_focused {
                        if step == -(GRID_COLUMNS as isize) {
                            return iced::Task::none();
                        }

                        let next = (self.result_focus.unwrap_or(0) as isize + step)
                            .clamp(0, apps_len.saturating_sub(1) as isize)
                            as usize;
                        self.search_focused = false;
                        self.result_focus = Some(next);
                        return iced::widget::operation::focus(result_id(next));
                    }

                    if let Some(index) = self.result_focus {
                        if step == -(GRID_COLUMNS as isize) && index < GRID_COLUMNS {
                            self.result_focus = None;
                            self.search_focused = true;
                            return iced::widget::operation::focus(SEARCH_INPUT_ID).chain(
                                iced::widget::operation::move_cursor_to_end(SEARCH_INPUT_ID),
                            );
                        }

                        let next = (index as isize + step)
                            .clamp(0, apps_len.saturating_sub(1) as isize)
                            as usize;
                        self.result_focus = Some(next);
                        return iced::widget::operation::focus(result_id(next));
                    }

                    let first_index = if step < 0 { apps_len - 1 } else { 0 };
                    self.search_focused = false;
                    self.result_focus = Some(first_index);
                    return iced::widget::operation::focus(result_id(first_index));
                } else if !self.search_focused
                    && key == iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter)
                    && let Some(index) = self.result_focus
                {
                    return self.launch_index(index);
                }
            }
            Message::Keyboard(_) => {}
            Message::LaunchFocused => return self.launch_focused(),
            Message::Launch(index) => {
                return self.launch_index(index);
            }
        }
        iced::Task::none()
    }

    fn launch_focused(&mut self) -> iced::Task<Message> {
        let index = self.result_focus.unwrap_or(0);
        self.launch_index(index)
    }

    fn launch_index(&mut self, index: usize) -> iced::Task<Message> {
        let Some(app) = self.filtered_apps().get(index).cloned() else {
            return iced::Task::none();
        };
        match launch(app) {
            Ok(()) => {
                self.status = format!("Started {}", app.name);
                close_window()
            }
            Err(error) => {
                self.status = format!("Could not start {}: {error}", app.name);
                iced::Task::none()
            }
        }
    }

    fn filtered_apps(&self) -> Vec<&AppEntry> {
        let query = self.query.trim().to_lowercase();
        self.apps
            .iter()
            .filter(|app| query.is_empty() || app.search_text.contains(&query))
            .collect()
    }

    fn view(&self) -> Element<'_, Message> {
        let apps = self.filtered_apps();
        let mut cards = grid::Grid::new().fluid(160).spacing(14);
        for (index, app) in apps.iter().enumerate() {
            let hinted = self.search_focused
                && !self.query.trim().is_empty()
                && self.result_focus.is_none()
                && index == 0;
            let keyboard_focused = self.result_focus == Some(index);
            cards = cards.push(app_card(
                app,
                index,
                hinted || keyboard_focused,
                keyboard_focused,
            ));
        }

        let search = text_input("Search applications…", &self.query)
            .id(SEARCH_INPUT_ID)
            .on_input(Message::SearchChanged)
            .on_submit(Message::LaunchFocused)
            .padding([13, 16])
            .size(16)
            .style(|theme: &Theme, status| {
                let mut style = text_input::default(theme, status);
                style.background = Background::Color(TILE);
                style.border = Border {
                    color: Color::from_rgba(0.55, 0.72, 0.95, 0.23),
                    width: 1.0,
                    radius: 14.0.into(),
                };
                style
            });

        let input_hint = self.search_focused && !self.query.trim().is_empty();
        let search = search
            .style(move |theme: &Theme, status| {
                let mut style = text_input::default(theme, status);
                style.background = Background::Color(TILE);
                style.border = Border {
                    color: if input_hint {
                        ACCENT
                    } else {
                        Color::from_rgba(0.55, 0.72, 0.95, 0.23)
                    },
                    width: 1.0,
                    radius: 14.0.into(),
                };
                style
            })
            .width(Length::Fill);
        let close_button = iced::widget::tooltip(
            button(
                container(text("×").size(24).color(TEXT))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill),
            )
            .width(Length::Fixed(48.0))
            .height(Length::Fixed(48.0))
            .padding(0)
            .on_press(Message::CloseLauncher)
            .style(|_theme: &Theme, status| button::Style {
                background: Some(Background::Color(match status {
                    button::Status::Hovered | button::Status::Pressed => TILE_HOVER,
                    _ => TILE,
                })),
                text_color: TEXT,
                border: Border {
                    color: Color::from_rgba(0.55, 0.72, 0.95, 0.23),
                    width: 1.0,
                    radius: 14.0.into(),
                },
                ..Default::default()
            }),
            "Close launcher",
            iced::widget::tooltip::Position::Bottom,
        );
        let count = format!("{} APPLICATIONS", apps.len());
        let footer = row![
            text(count).size(11).color(MUTED),
            iced::widget::Space::new().width(Length::Fill),
            text(self.status.as_str()).size(12).color(ACCENT),
        ]
        .align_y(iced::Alignment::Center);

        container(
            column![
                row![search, close_button]
                    .spacing(10)
                    .align_y(iced::Alignment::Center),
                scrollable(container(cards).width(Length::Fill).padding(right(16)))
                    .height(Length::Fill),
                footer
            ]
            .spacing(20)
            .padding(26),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(GLASS)),
            border: Border {
                color: Color::from_rgba(0.62, 0.77, 0.98, 0.28),
                width: 1.0,
                radius: 26.0.into(),
            },
            ..Default::default()
        })
        .padding(10)
        .into()
    }
}

fn close_window() -> iced::Task<Message> {
    iced::window::latest().and_then(iced::window::close)
}

fn app_card<'a>(
    app: &&'a AppEntry,
    index: usize,
    highlighted: bool,
    keyboard_focused: bool,
) -> Element<'a, Message> {
    let icon: Element<'a, Message> = match app.icon.as_deref() {
        Some(path) if path.extension().is_some_and(|extension| extension == "svg") => {
            svg(svg::Handle::from_path(path))
                .width(Length::Fixed(54.0))
                .height(Length::Fixed(54.0))
                .into()
        }
        Some(path) => image(image::Handle::from_path(path))
            .width(Length::Fixed(54.0))
            .height(Length::Fixed(54.0))
            .into(),
        None => text(monogram(&app.name)).size(26).color(ACCENT).into(),
    };

    let contents = column![
        container(icon)
            .width(Length::Fixed(72.0))
            .height(Length::Fixed(72.0))
            .center_x(Length::Fill)
            .center_y(Length::Fill),
        text(app.name.as_str())
            .size(14)
            .color(TEXT)
            .align_x(Horizontal::Center)
            .width(Length::Fill),
    ]
    .spacing(12)
    .align_x(iced::Alignment::Center);

    let card = button(contents)
        .width(Length::Fill)
        .height(Length::Fixed(150.0))
        .padding(14)
        .on_press(Message::Launch(index))
        .style(move |_theme: &Theme, status| {
            let background = if highlighted {
                TILE_FOCUSED
            } else {
                match status {
                    button::Status::Hovered | button::Status::Pressed => TILE_HOVER,
                    _ => TILE,
                }
            };
            button::Style {
                background: Some(Background::Color(background)),
                text_color: TEXT,
                border: Border {
                    color: if keyboard_focused {
                        ACCENT
                    } else {
                        Color::from_rgba(0.58, 0.73, 0.94, 0.22)
                    },
                    width: if keyboard_focused { 2.0 } else { 1.0 },
                    radius: 20.0.into(),
                },
                ..Default::default()
            }
        });

    FocusableCard::new(card, result_id(index)).into()
}

fn monogram(name: &str) -> String {
    name.split_whitespace()
        .take(2)
        .filter_map(|word| word.chars().next())
        .collect::<String>()
        .to_uppercase()
}

fn load_apps() -> Vec<AppEntry> {
    let locales = get_languages_from_env();
    let mut seen = HashSet::new();
    let mut apps = Iter::new(default_paths())
        .entries(Some(&locales))
        .filter_map(|entry| app_from_desktop_entry(entry, &locales, &mut seen))
        .collect::<Vec<_>>();
    apps.sort_by_key(|app| app.name.to_lowercase());
    apps
}

fn app_from_desktop_entry(
    entry: DesktopEntry,
    locales: &[String],
    seen: &mut HashSet<String>,
) -> Option<AppEntry> {
    if entry.type_() != Some("Application")
        || entry.no_display()
        || entry.hidden()
        || !seen.insert(entry.id().to_owned())
    {
        return None;
    }

    let name = entry.name(locales)?.into_owned();
    let command = entry.parse_exec().ok()?;
    if command.is_empty()
        || entry
            .try_exec()
            .is_some_and(|program| !program_exists(program))
    {
        return None;
    }

    let mut search_terms = vec![name.clone(), entry.id().to_owned()];
    if let Some(generic_name) = entry.generic_name(locales) {
        search_terms.push(generic_name.into_owned());
    }
    if let Some(keywords) = entry.keywords(locales) {
        search_terms.extend(keywords.into_iter().map(|keyword| keyword.into_owned()));
    }
    // Include both parsed command tokens (the executable and arguments) and the
    // original Exec value so users can find apps by executable or command name.
    search_terms.extend(command.iter().cloned());
    if let Some(exec) = entry.exec() {
        search_terms.push(exec.to_owned());
    }
    let search_text = search_terms.join(" ").to_lowercase();

    Some(AppEntry {
        name,
        search_text,
        icon: entry.icon().and_then(find_icon),
        command,
        working_directory: entry.path().map(PathBuf::from),
        terminal: entry.terminal(),
    })
}

fn program_exists(program: &str) -> bool {
    let path = Path::new(program);
    if path.components().count() > 1 {
        return path.is_file();
    }
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .any(|directory| directory.join(program).is_file())
}

fn find_icon(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.is_absolute() && path.is_file() {
        return Some(path.to_owned());
    }

    let mut roots = Vec::new();
    for data_home in
        std::env::split_paths(&std::env::var_os("XDG_DATA_HOME").unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(|home| PathBuf::from(home).join(".local/share").into_os_string())
                .unwrap_or_default()
        }))
    {
        roots.push(data_home.join("icons"));
    }
    for data_dir in std::env::split_paths(
        &std::env::var_os("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into()),
    ) {
        roots.push(data_dir.join("icons"));
    }
    roots.push(PathBuf::from("/usr/share/pixmaps"));

    for root in roots {
        for theme in ["hicolor", "breeze", "Adwaita"] {
            for size in [
                "scalable/apps",
                "256x256/apps",
                "128x128/apps",
                "64x64/apps",
                "48x48/apps",
                "32x32/apps",
                "apps",
            ] {
                for extension in ["svg", "png", "xpm"] {
                    let candidate = root
                        .join(theme)
                        .join(size)
                        .join(format!("{name}.{extension}"));
                    if candidate.is_file() {
                        return Some(candidate);
                    }
                }
            }
        }
        for extension in ["svg", "png", "xpm"] {
            let candidate = root.join(format!("{name}.{extension}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn launch(app: &AppEntry) -> std::io::Result<()> {
    let (program, arguments) = app.command.split_first().expect("validated command");
    let mut command = if app.terminal {
        let terminal = std::env::var("TERMINAL").unwrap_or_else(|_| "x-terminal-emulator".into());
        let mut command = Command::new(terminal);
        command.arg("-e").arg(program).args(arguments);
        command
    } else {
        let mut command = Command::new(program);
        command.args(arguments);
        command
    };

    if let Some(directory) = &app.working_directory {
        command.current_dir(directory);
    }
    // Do not replace WAYLAND_DISPLAY: children inherit the forwarded session display.
    command.spawn().map(|_| ())
}

fn main() -> iced::Result {
    iced::application(
        || {
            (
                Launcher::new(),
                iced::widget::operation::focus(SEARCH_INPUT_ID),
            )
        },
        Launcher::update,
        Launcher::view,
    )
    .subscription(|_| {
        iced::event::listen_raw(|event, _, _| match event {
            iced::Event::Keyboard(keyboard_event) => Some(Message::Keyboard(keyboard_event)),
            iced::Event::Window(iced::window::Event::Unfocused) => Some(Message::WindowUnfocused),
            iced::Event::Mouse(iced::mouse::Event::ButtonPressed(_))
            | iced::Event::Touch(iced::touch::Event::FingerPressed { .. }) => {
                Some(Message::RefreshSearchFocus)
            }
            _ => None,
        })
    })
    .title("Spacetop App Launcher")
    .window_size((920.0, 680.0))
    .window(iced::window::Settings {
        decorations: false,
        ..Default::default()
    })
    .transparent(true)
    .centered()
    .theme(Theme::Dark)
    .style(|_state, _theme| iced::theme::Style {
        background_color: Color::TRANSPARENT,
        text_color: TEXT,
    })
    .run()
}

#[cfg(test)]
mod tests {
    use super::monogram;

    #[test]
    fn monogram_uses_up_to_two_words() {
        assert_eq!(monogram("Web Browser"), "WB");
        assert_eq!(monogram("Calculator"), "C");
    }
}
