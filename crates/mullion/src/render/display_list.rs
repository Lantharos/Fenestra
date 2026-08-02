use crate::style::Color;

#[derive(Clone, Debug, PartialEq)]
pub struct DisplayList {
    pub background: Color,
    pub commands: Vec<DisplayCommand>,
    pub hovered_region: Option<String>,
    pub pressed_region: Option<String>,
    pub focused_region: Option<String>,
}

impl DisplayList {
    pub fn new(background: Color) -> Self {
        Self {
            background,
            commands: Vec::new(),
            hovered_region: None,
            pressed_region: None,
            focused_region: None,
        }
    }

    pub fn push(&mut self, command: impl Into<DisplayCommand>) {
        self.commands.push(command.into());
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum DisplayCommand {
    Rect(RectCommand),
    RoundedRect(RoundedRectCommand),
    Text(TextCommand),
    Image(ImageCommand),
}

#[derive(Clone, Debug, PartialEq)]
pub struct RectCommand {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub color: Color,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RoundedRectCommand {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub radius: f32,
    pub color: Color,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextCommand {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub size: f32,
    pub line_height: f32,
    pub color: Color,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImageCommand {
    pub id: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub opacity: f32,
}

impl From<RectCommand> for DisplayCommand {
    fn from(command: RectCommand) -> Self {
        Self::Rect(command)
    }
}

impl From<RoundedRectCommand> for DisplayCommand {
    fn from(command: RoundedRectCommand) -> Self {
        Self::RoundedRect(command)
    }
}

impl From<TextCommand> for DisplayCommand {
    fn from(command: TextCommand) -> Self {
        Self::Text(command)
    }
}

impl From<ImageCommand> for DisplayCommand {
    fn from(command: ImageCommand) -> Self {
        Self::Image(command)
    }
}
