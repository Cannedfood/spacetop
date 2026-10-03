use iced::{
    Background, Border, Color, Element, Length, Theme,
    alignment::Horizontal,
    widget::{button, column, container, row, scrollable, slider, text, text_input},
};
use spacetop_config::AppConfig;

const TEXT: Color = Color::from_rgb(0.92, 0.95, 0.96);
const MUTED: Color = Color::from_rgb(0.60, 0.69, 0.72);
const ACCENT: Color = Color::from_rgb(0.34, 0.82, 0.72);
const SURFACE: Color = Color::from_rgb(0.11, 0.15, 0.17);
const INPUT: Color = Color::from_rgb(0.07, 0.10, 0.12);
const BORDER: Color = Color::from_rgba(0.52, 0.73, 0.73, 0.22);

#[derive(Debug, Clone)]
enum Message {
    BackgroundChanged(String),
    FloorHeightChanged(String),
    AlbedoChanged(usize, String),
    RoughnessChanged(f32),
    ReflectanceChanged(f32),
    TransparencyChanged(f32),
    RayCountChanged(u32),
    WindowDistanceChanged(String),
    CursorDistanceChanged(String),
    Reload,
    Save,
}

struct SettingsApp {
    config: AppConfig,
    floor_height: String,
    albedo: [String; 3],
    window_distance: String,
    cursor_distance: String,
    status: String,
    status_is_error: bool,
}

impl SettingsApp {
    fn new() -> Self {
        match AppConfig::load() {
            Ok(config) => Self::from_config(config, "Configuration loaded".into(), false),
            Err(error) => Self::from_config(
                AppConfig::default(),
                format!("Could not load config: {error:#}"),
                true,
            ),
        }
    }

    fn from_config(config: AppConfig, status: String, status_is_error: bool) -> Self {
        let floor_height = format!("{}", config.floor.height_m);
        let albedo = config.floor.albedo.map(|channel| format!("{channel:.3}"));
        let window_distance = format!("{}", config.window.default_distance_m);
        let cursor_distance = format!("{}", config.cursor.default_distance_m);
        Self {
            config,
            floor_height,
            albedo,
            window_distance,
            cursor_distance,
            status,
            status_is_error,
        }
    }

    fn update(&mut self, message: Message) -> iced::Task<Message> {
        let editing = !matches!(&message, Message::Reload | Message::Save);
        match message {
            Message::BackgroundChanged(image) => self.config.background.image = image,
            Message::FloorHeightChanged(value) => self.floor_height = value,
            Message::AlbedoChanged(channel, value) => self.albedo[channel] = value,
            Message::RoughnessChanged(value) => self.config.floor.roughness = value,
            Message::ReflectanceChanged(value) => self.config.floor.reflectance = value,
            Message::TransparencyChanged(value) => self.config.floor.transparency = value,
            Message::RayCountChanged(value) => self.config.floor.ray_count = value,
            Message::WindowDistanceChanged(value) => self.window_distance = value,
            Message::CursorDistanceChanged(value) => self.cursor_distance = value,
            Message::Reload => match AppConfig::load() {
                Ok(config) => {
                    *self = Self::from_config(config, "Configuration reloaded".into(), false)
                }
                Err(error) => {
                    self.status = format!("Could not reload config: {error:#}");
                    self.status_is_error = true;
                }
            },
            Message::Save => self.save(),
        }
        if editing {
            self.status = "Unsaved changes".into();
            self.status_is_error = false;
        }
        iced::Task::none()
    }

    fn save(&mut self) {
        let config = match self.config_from_fields() {
            Ok(config) => config,
            Err(error) => {
                self.status = error;
                self.status_is_error = true;
                return;
            }
        };
        let result = AppConfig::path().and_then(|path| config.save_to(&path));
        match result {
            Ok(()) => {
                self.config = config;
                self.status = "Saved. Spacetop will reload shortly.".into();
                self.status_is_error = false;
            }
            Err(error) => {
                self.status = format!("Could not save config: {error:#}");
                self.status_is_error = true;
            }
        }
    }

    fn config_from_fields(&self) -> Result<AppConfig, String> {
        let mut config = self.config.clone();
        config.floor.height_m = parse_number("Fallback floor height", &self.floor_height)?;
        for (index, value) in self.albedo.iter().enumerate() {
            config.floor.albedo[index] = parse_number("Albedo channel", value)?;
        }
        config.window.default_distance_m =
            parse_number("Default window distance", &self.window_distance)?;
        config.cursor.default_distance_m =
            parse_number("Default cursor distance", &self.cursor_distance)?;
        config
            .validate()
            .map_err(|error| format!("Invalid settings: {error}"))?;
        Ok(config)
    }

    fn view(&self) -> Element<'_, Message> {
        let environment = section(
            "BACKGROUND",
            column![
                text("IMAGE SOURCE").size(11).color(MUTED),
                styled_input(
                    "random or /path/to/image.exr",
                    &self.config.background.image,
                    Message::BackgroundChanged,
                ),
            ]
            .spacing(8),
        );

        let floor_height = labeled_input(
            "FALLBACK HEIGHT (M)",
            &self.floor_height,
            "-1.3",
            Message::FloorHeightChanged,
        );
        let albedo_fields = row![
            channel_input("R", 0, &self.albedo[0]),
            channel_input("G", 1, &self.albedo[1]),
            channel_input("B", 2, &self.albedo[2]),
            color_swatch(&self.albedo),
        ]
        .spacing(12)
        .align_y(iced::Alignment::End);
        let floor = section(
            "FLOOR MATERIAL",
            column![
                row![
                    floor_height,
                    column![text("ALBEDO").size(11).color(MUTED), albedo_fields].spacing(8)
                ]
                .spacing(18)
                .align_y(iced::Alignment::End),
                slider_row(
                    "ROUGHNESS",
                    self.config.floor.roughness,
                    Message::RoughnessChanged,
                ),
                slider_row(
                    "REFLECTANCE",
                    self.config.floor.reflectance,
                    Message::ReflectanceChanged,
                ),
                slider_row(
                    "TRANSPARENCY",
                    self.config.floor.transparency,
                    Message::TransparencyChanged,
                ),
                row![
                    text("REFLECTION RAYS")
                        .size(11)
                        .color(MUTED)
                        .width(Length::Fixed(126.0)),
                    slider(
                        1..=64,
                        self.config.floor.ray_count,
                        Message::RayCountChanged
                    )
                    .step(1_u32)
                    .width(Length::Fill),
                    text(self.config.floor.ray_count.to_string())
                        .size(13)
                        .color(TEXT)
                        .width(Length::Fixed(34.0))
                        .align_x(Horizontal::Right),
                ]
                .spacing(12)
                .align_y(iced::Alignment::Center),
            ]
            .spacing(18),
        );

        let placement = row![
            section(
                "WINDOWS",
                labeled_input(
                    "DEFAULT DISTANCE (M)",
                    &self.window_distance,
                    "1.6",
                    Message::WindowDistanceChanged,
                ),
            ),
            section(
                "CURSOR",
                labeled_input(
                    "DEFAULT DISTANCE (M)",
                    &self.cursor_distance,
                    "1.6",
                    Message::CursorDistanceChanged,
                ),
            ),
        ]
        .spacing(14)
        .align_y(iced::Alignment::Start);

        let status_color = if self.status_is_error {
            Color::from_rgb(0.98, 0.47, 0.39)
        } else {
            ACCENT
        };
        let footer = row![
            text(self.status.as_str())
                .size(12)
                .color(status_color)
                .width(Length::Fill),
            button(text("Reload").size(13))
                .on_press(Message::Reload)
                .style(quiet_button),
            button(text("Save changes").size(13))
                .on_press(Message::Save)
                .style(accent_button),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);

        container(
            column![
                row![
                    column![
                        text("SPACE SETTINGS").size(12).color(ACCENT),
                        text("Configuration").size(25).color(TEXT),
                        text("~/.config/spacetop/config.toml").size(12).color(MUTED),
                    ]
                    .spacing(4),
                    iced::widget::Space::new().width(Length::Fill),
                ]
                .align_y(iced::Alignment::Center),
                scrollable(column![environment, floor, placement].spacing(14)).height(Length::Fill),
                footer,
            ]
            .spacing(18)
            .padding(24),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(Color::from_rgb(0.055, 0.075, 0.085))),
            ..Default::default()
        })
        .into()
    }
}

fn parse_number(label: &str, value: &str) -> Result<f32, String> {
    value
        .trim()
        .parse::<f32>()
        .map_err(|_| format!("{label} must be a number"))
}

fn section<'a>(title: &'a str, content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(column![text(title).size(11).color(ACCENT), content.into()].spacing(16))
        .width(Length::Fill)
        .padding(18)
        .style(|_| container::Style {
            background: Some(Background::Color(SURFACE)),
            border: Border {
                color: BORDER,
                width: 1.0,
                radius: 8.0.into(),
            },
            ..Default::default()
        })
        .into()
}

fn labeled_input<'a>(
    label: &'a str,
    value: &'a str,
    placeholder: &'a str,
    on_input: impl Fn(String) -> Message + 'static,
) -> Element<'a, Message> {
    column![
        text(label).size(11).color(MUTED),
        styled_input(placeholder, value, on_input),
    ]
    .spacing(8)
    .width(Length::Fill)
    .into()
}

fn styled_input<'a>(
    placeholder: &'a str,
    value: &'a str,
    on_input: impl Fn(String) -> Message + 'static,
) -> Element<'a, Message> {
    text_input(placeholder, value)
        .on_input(on_input)
        .padding([9, 11])
        .size(14)
        .style(|theme, status| {
            let mut style = text_input::default(theme, status);
            style.background = Background::Color(INPUT);
            style.border = Border {
                color: BORDER,
                width: 1.0,
                radius: 6.0.into(),
            };
            style
        })
        .into()
}

fn channel_input<'a>(label: &'a str, channel: usize, value: &'a str) -> Element<'a, Message> {
    column![
        text(label).size(11).color(MUTED),
        styled_input("0.00", value, move |value| Message::AlbedoChanged(
            channel, value
        )),
    ]
    .spacing(8)
    .width(Length::Fixed(78.0))
    .into()
}

fn color_swatch(channels: &[String; 3]) -> Element<'static, Message> {
    let values = channels
        .each_ref()
        .map(|channel| channel.parse::<f32>().unwrap_or_default().clamp(0.0, 1.0));
    let color = Color::from_rgb(values[0], values[1], values[2]);
    container(text(""))
        .width(Length::Fixed(42.0))
        .height(Length::Fixed(36.0))
        .style(move |_| container::Style {
            background: Some(Background::Color(color)),
            border: Border {
                color: BORDER,
                width: 1.0,
                radius: 6.0.into(),
            },
            ..Default::default()
        })
        .into()
}

fn slider_row<'a>(
    label: &'a str,
    value: f32,
    on_change: impl Fn(f32) -> Message + 'static,
) -> Element<'a, Message> {
    row![
        text(label)
            .size(11)
            .color(MUTED)
            .width(Length::Fixed(126.0)),
        slider(0.0..=1.0, value, on_change)
            .step(0.01_f32)
            .width(Length::Fill),
        text(format!("{value:.2}"))
            .size(13)
            .color(TEXT)
            .width(Length::Fixed(34.0))
            .align_x(Horizontal::Right),
    ]
    .spacing(12)
    .align_y(iced::Alignment::Center)
    .into()
}

fn quiet_button(_theme: &Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(Background::Color(SURFACE)),
        text_color: TEXT,
        border: Border {
            color: BORDER,
            width: 1.0,
            radius: 6.0.into(),
        },
        ..Default::default()
    }
}

fn accent_button(_theme: &Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(Background::Color(ACCENT)),
        text_color: Color::from_rgb(0.035, 0.10, 0.095),
        border: Border {
            color: ACCENT,
            width: 1.0,
            radius: 6.0.into(),
        },
        ..Default::default()
    }
}

fn main() -> iced::Result {
    iced::application(SettingsApp::new, SettingsApp::update, SettingsApp::view)
        .title("Space Settings")
        .window_size((760.0, 820.0))
        .centered()
        .theme(Theme::Dark)
        .run()
}
