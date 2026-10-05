use iced::{
    Color, Element, Length, Point, Size, Subscription, Theme,
    alignment::Horizontal,
    widget::{canvas, column, container, row, stack, text},
};

#[derive(Debug, Clone, Copy)]
enum Message {
    CursorMoved(Point),
    CursorLeft,
    WindowResized(Size),
}

struct CursorPosition {
    position: Option<Point>,
    window_size: Size,
}

impl Default for CursorPosition {
    fn default() -> Self {
        Self {
            position: None,
            window_size: Size::new(560.0, 320.0),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct CursorOverlay {
    position: Option<Point>,
}

impl canvas::Program<Message> for CursorOverlay {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &Theme,
        bounds: iced::Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());

        if let Some(position) = self.position {
            let point = Point::new(position.x - bounds.x, position.y - bounds.y);
            let crosshair = canvas::Path::new(|path| {
                path.move_to(Point::new(point.x - 16.0, point.y));
                path.line_to(Point::new(point.x - 6.0, point.y));
                path.move_to(Point::new(point.x + 6.0, point.y));
                path.line_to(Point::new(point.x + 16.0, point.y));
                path.move_to(Point::new(point.x, point.y - 16.0));
                path.line_to(Point::new(point.x, point.y - 6.0));
                path.move_to(Point::new(point.x, point.y + 6.0));
                path.line_to(Point::new(point.x, point.y + 16.0));
            });
            let ring = canvas::Path::circle(point, 4.0);

            frame.stroke(
                &crosshair,
                canvas::Stroke::default()
                    .with_color(Color::from_rgb(0.02, 0.04, 0.04))
                    .with_width(4.0),
            );
            frame.stroke(
                &crosshair,
                canvas::Stroke::default()
                    .with_color(Color::from_rgb(0.45, 0.96, 0.79))
                    .with_width(2.0),
            );
            frame.stroke(
                &ring,
                canvas::Stroke::default()
                    .with_color(Color::from_rgb(0.02, 0.04, 0.04))
                    .with_width(3.0),
            );
            frame.fill(&ring, Color::from_rgb(0.97, 0.72, 0.36));
        }

        vec![frame.into_geometry()]
    }
}

impl CursorPosition {
    fn update(&mut self, message: Message) {
        match message {
            Message::CursorMoved(position) => self.position = Some(position),
            Message::CursorLeft => self.position = None,
            Message::WindowResized(size) => self.window_size = size,
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let (coordinates, status) = match self.position {
            Some(position) => (
                format!("{:.0}  /  {:.0}", position.x, position.y),
                "POINTER INSIDE WINDOW",
            ),
            None => ("---  /  ---".into(), "POINTER OUTSIDE WINDOW"),
        };
        let (left, top, right, bottom) = self.position.map_or_else(
            || ("---".into(), "---".into(), "---".into(), "---".into()),
            |position| {
                (
                    format!("{:.0}", position.x),
                    format!("{:.0}", position.y),
                    format!("{:.0}", self.window_size.width - position.x),
                    format!("{:.0}", self.window_size.height - position.y),
                )
            },
        );

        stack![
            container(
                column![
                    text("CURSOR POSITION")
                        .size(15)
                        .color(Color::from_rgb(0.58, 0.78, 0.75)),
                    text(coordinates)
                        .size(58)
                        .color(Color::from_rgb(0.93, 0.96, 0.92)),
                    text("X  /  Y     WINDOW PIXELS")
                        .size(13)
                        .color(Color::from_rgb(0.64, 0.69, 0.67)),
                    row![
                        text(format!("LEFT  {left} px")).size(15),
                        text(format!("RIGHT  {right} px")).size(15),
                    ]
                    .spacing(24),
                    row![
                        text(format!("TOP  {top} px")).size(15),
                        text(format!("BOTTOM  {bottom} px")).size(15),
                    ]
                    .spacing(24),
                    text(format!(
                        "WINDOW  {:.0} x {:.0} px",
                        self.window_size.width, self.window_size.height
                    ))
                    .size(13)
                    .color(Color::from_rgb(0.64, 0.69, 0.67)),
                    text(status)
                        .size(12)
                        .color(Color::from_rgb(0.81, 0.73, 0.48)),
                ]
                .spacing(14)
                .align_x(Horizontal::Center),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center(Length::Fill)
            .style(|_| container::Style {
                background: Some(Color::from_rgb(0.08, 0.11, 0.12).into()),
                ..Default::default()
            }),
            canvas::Canvas::new(CursorOverlay {
                position: self.position,
            })
            .width(Length::Fill)
            .height(Length::Fill),
        ]
        .into()
    }
}

fn subscription(_: &CursorPosition) -> Subscription<Message> {
    iced::event::listen_raw(|event, _, _| match event {
        iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
            Some(Message::CursorMoved(position))
        }
        iced::Event::Mouse(iced::mouse::Event::CursorLeft) => Some(Message::CursorLeft),
        iced::Event::Window(iced::window::Event::Resized(size)) => {
            Some(Message::WindowResized(size))
        }
        _ => None,
    })
}

fn main() -> iced::Result {
    iced::application(
        CursorPosition::default,
        CursorPosition::update,
        CursorPosition::view,
    )
    .subscription(subscription)
    .title("Cursor Position")
    .window_size((560.0, 320.0))
    .centered()
    .theme(Theme::Dark)
    .run()
}
