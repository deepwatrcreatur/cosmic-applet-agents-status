mod app;

pub fn run() -> cosmic::iced::Result {
    cosmic::applet::run::<app::AgentsApplet>(())
}
