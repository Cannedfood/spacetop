use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    process::Command,
};

use freedesktop_desktop_entry::{DesktopEntry, Iter, default_paths, get_languages_from_env};
use iced::{
    Background, Border, Color, Element, Length, Theme,
    alignment::Horizontal,
    widget::{button, column, container, grid, image, row, scrollable, svg, text, text_input},
};

const ACCENT: Color = Color::from_rgb(0.40, 0.78, 0.98);
const TEXT: Color = Color::from_rgb(0.94, 0.96, 1.0);
const MUTED: Color = Color::from_rgb(0.66, 0.72, 0.81);
const GLASS: Color = Color::from_rgba(0.035, 0.055, 0.09, 0.50);
const TILE: Color = Color::from_rgba(0.12, 0.16, 0.23, 0.76);
const TILE_HOVER: Color = Color::from_rgba(0.19, 0.26, 0.36, 0.94);

#[derive(Debug, Clone)]
struct AppEntry {
    id: String,
    name: String,
    icon: Option<PathBuf>,
    command: Vec<String>,
    working_directory: Option<PathBuf>,
    terminal: bool,
}

#[derive(Debug, Clone)]
enum Message {
    SearchChanged(String),
    Launch(usize),
}

struct Launcher {
    apps: Vec<AppEntry>,
    query: String,
    status: String,
}

impl Launcher {
    fn new() -> Self {
        let apps = load_apps();

        Self {
            apps,
            query: String::new(),
            status: String::new(),
        }
    }

    fn update(&mut self, message: Message) {
        match message {
            Message::SearchChanged(query) => self.query = query,
            Message::Launch(index) => {
                let Some(app) = self.filtered_apps().get(index).cloned() else {
                    return;
                };
                match launch(app) {
                    Ok(()) => self.status = format!("Started {}", app.name),
                    Err(error) => self.status = format!("Could not start {}: {error}", app.name),
                }
            }
        }
    }

    fn filtered_apps(&self) -> Vec<&AppEntry> {
        let query = self.query.trim().to_lowercase();
        self.apps
            .iter()
            .filter(|app| {
                query.is_empty()
                    || app.name.to_lowercase().contains(&query)
                    || app.id.to_lowercase().contains(&query)
            })
            .collect()
    }

    fn view(&self) -> Element<'_, Message> {
        let apps = self.filtered_apps();
        let mut cards = grid::Grid::new().fluid(160).spacing(14);
        for (index, app) in apps.iter().enumerate() {
            cards = cards.push(app_card(app, index));
        }

        let search = text_input("Search applications…", &self.query)
            .on_input(Message::SearchChanged)
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

        let count = format!("{} APPLICATIONS", apps.len());
        let footer = row![
            text(count).size(11).color(MUTED),
            iced::widget::Space::new().width(Length::Fill),
            text(self.status.as_str()).size(12).color(ACCENT),
        ]
        .align_y(iced::Alignment::Center);

        container(
            column![search, scrollable(cards).height(Length::Fill), footer]
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

fn app_card<'a>(app: &&'a AppEntry, index: usize) -> Element<'a, Message> {
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

    button(contents)
        .width(Length::Fill)
        .height(Length::Fixed(150.0))
        .padding(14)
        .on_press(Message::Launch(index))
        .style(|_theme: &Theme, status| {
            let background = match status {
                button::Status::Hovered | button::Status::Pressed => TILE_HOVER,
                _ => TILE,
            };
            button::Style {
                background: Some(Background::Color(background)),
                text_color: TEXT,
                border: Border {
                    color: Color::from_rgba(0.58, 0.73, 0.94, 0.22),
                    width: 1.0,
                    radius: 20.0.into(),
                },
                ..Default::default()
            }
        })
        .into()
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

    Some(AppEntry {
        id: entry.id().to_owned(),
        name,
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
    iced::application(Launcher::new, Launcher::update, Launcher::view)
        .title("Spacetop App Launcher")
        .window_size((920.0, 680.0))
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
