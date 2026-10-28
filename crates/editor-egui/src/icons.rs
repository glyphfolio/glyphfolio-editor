//! Small original line icons painted directly by egui, without image assets.

use egui::{Color32, FontId, Painter, Rect, Stroke, pos2};

/// Native controls share one simple visual vocabulary. Coordinates are drawn
/// locally; no editor framework, SVG loader, or icon-font runtime is required.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Undo,
    Redo,
    Bold,
    Italic,
    Strikethrough,
    TaskList,
    Table,
    Link,
    InlineCode,
    BulletList,
    OrderedList,
    Blockquote,
    CodeFence,
    Indent,
    Outdent,
    Save,
    ChevronDown,
    ChevronRight,
    More,
    Settings,
    File,
    Folder,
    Inbox,
    Star,
    Trash,
    Tag,
    Search,
    Check,
    Close,
    Info,
    Sun,
    Moon,
    Cube,
}

impl Icon {
    pub fn draw(self, painter: &Painter, bounds: Rect, color: Color32) {
        let at = |x: f32, y: f32| {
            pos2(
                bounds.left() + x * bounds.width() / 24.0,
                bounds.top() + y * bounds.height() / 24.0,
            )
        };
        let stroke = Stroke::new(1.75, color);
        let line = |a: (f32, f32), b: (f32, f32)| {
            painter.line_segment([at(a.0, a.1), at(b.0, b.1)], stroke);
        };
        let path = |points: &[(f32, f32)]| {
            painter.add(egui::Shape::line(
                points.iter().map(|(x, y)| at(*x, *y)).collect(),
                stroke,
            ));
        };
        let curve = |points: [(f32, f32); 4]| {
            painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                points.map(|(x, y)| at(x, y)),
                false,
                Color32::TRANSPARENT,
                stroke,
            ));
        };
        let circle = |x: f32, y: f32, radius: f32| {
            painter.circle_stroke(at(x, y), radius * bounds.width() / 24.0, stroke);
        };
        match self {
            Self::Undo => {
                path(&[(8.0, 5.0), (3.0, 10.0), (8.0, 15.0)]);
                curve([(3.0, 10.0), (20.0, 3.0), (23.0, 21.0), (12.0, 20.0)]);
            }
            Self::Redo => {
                path(&[(16.0, 5.0), (21.0, 10.0), (16.0, 15.0)]);
                curve([(21.0, 10.0), (4.0, 3.0), (1.0, 21.0), (12.0, 20.0)]);
            }
            Self::Link => {
                curve([(9.0, 16.0), (0.0, 24.0), (0.0, 9.0), (9.0, 7.0)]);
                curve([(15.0, 8.0), (24.0, 0.0), (24.0, 15.0), (15.0, 17.0)]);
                line((8.0, 16.0), (16.0, 8.0));
            }
            Self::Strikethrough => {
                curve([(18.0, 6.0), (7.0, 0.0), (3.0, 11.0), (12.0, 12.0)]);
                curve([(12.0, 12.0), (23.0, 14.0), (15.0, 24.0), (6.0, 18.0)]);
                line((3.0, 12.0), (21.0, 12.0));
            }
            Self::Table => {
                path(&[
                    (3.0, 4.0),
                    (21.0, 4.0),
                    (21.0, 20.0),
                    (3.0, 20.0),
                    (3.0, 4.0),
                ]);
                line((3.0, 10.0), (21.0, 10.0));
                line((3.0, 15.0), (21.0, 15.0));
                line((9.0, 4.0), (9.0, 20.0));
                line((15.0, 4.0), (15.0, 20.0));
            }
            Self::TaskList => {
                path(&[(3.0, 6.0), (5.0, 8.0), (9.0, 3.0)]);
                line((12.0, 6.0), (22.0, 6.0));
                path(&[(3.0, 17.0), (5.0, 19.0), (9.0, 14.0)]);
                line((12.0, 17.0), (22.0, 17.0));
            }
            Self::Bold => {
                line((7.0, 4.0), (7.0, 20.0));
                line((7.0, 4.0), (12.0, 4.0));
                curve([(12.0, 4.0), (20.0, 4.0), (20.0, 12.0), (12.0, 12.0)]);
                line((7.0, 12.0), (12.0, 12.0));
                curve([(12.0, 12.0), (21.0, 12.0), (21.0, 20.0), (12.0, 20.0)]);
                line((7.0, 20.0), (12.0, 20.0));
            }
            Self::Italic => {
                line((11.0, 4.0), (20.0, 4.0));
                line((15.5, 4.0), (8.5, 20.0));
                line((4.0, 20.0), (13.0, 20.0));
            }
            Self::InlineCode => {
                path(&[(8.0, 5.0), (2.0, 12.0), (8.0, 19.0)]);
                path(&[(16.0, 5.0), (22.0, 12.0), (16.0, 19.0)]);
            }
            Self::BulletList | Self::OrderedList => {
                for (index, y) in [5.0, 12.0, 19.0].into_iter().enumerate() {
                    if self == Self::BulletList {
                        painter.circle_filled(at(3.5, y), bounds.width() / 14.0, color);
                    } else {
                        painter.text(
                            at(3.5, y),
                            egui::Align2::CENTER_CENTER,
                            index + 1,
                            FontId::proportional(bounds.height() * 0.46),
                            color,
                        );
                    }
                    line((9.0, y), (22.0, y));
                }
            }
            Self::Blockquote => {
                line((3.0, 3.0), (3.0, 21.0));
                line((9.0, 6.0), (21.0, 6.0));
                line((9.0, 12.0), (20.0, 12.0));
                line((9.0, 18.0), (17.0, 18.0));
            }
            Self::CodeFence => {
                path(&[(7.0, 2.0), (3.0, 2.0), (3.0, 22.0), (7.0, 22.0)]);
                path(&[(17.0, 2.0), (21.0, 2.0), (21.0, 22.0), (17.0, 22.0)]);
                path(&[(10.0, 8.0), (7.0, 12.0), (10.0, 16.0)]);
                path(&[(14.0, 8.0), (17.0, 12.0), (14.0, 16.0)]);
            }
            Self::Indent | Self::Outdent => {
                line((2.0, 3.0), (22.0, 3.0));
                line((11.0, 9.0), (22.0, 9.0));
                line((11.0, 15.0), (22.0, 15.0));
                line((2.0, 21.0), (22.0, 21.0));
                if self == Self::Indent {
                    path(&[(3.0, 8.0), (7.0, 12.0), (3.0, 16.0)]);
                } else {
                    path(&[(7.0, 8.0), (3.0, 12.0), (7.0, 16.0)]);
                }
            }
            Self::Save => {
                path(&[
                    (4.0, 3.0),
                    (17.0, 3.0),
                    (21.0, 7.0),
                    (21.0, 21.0),
                    (3.0, 21.0),
                    (3.0, 3.0),
                    (4.0, 3.0),
                ]);
                path(&[(7.0, 3.0), (7.0, 10.0), (16.0, 10.0), (16.0, 3.0)]);
                path(&[(7.0, 21.0), (7.0, 15.0), (17.0, 15.0), (17.0, 21.0)]);
            }
            Self::ChevronDown => path(&[(5.0, 9.0), (12.0, 16.0), (19.0, 9.0)]),
            Self::ChevronRight => path(&[(9.0, 5.0), (16.0, 12.0), (9.0, 19.0)]),
            Self::More => {
                for x in [4.0, 12.0, 20.0] {
                    painter.circle_filled(at(x, 12.0), bounds.width() / 11.0, color);
                }
            }
            Self::Settings => {
                circle(12.0, 12.0, 6.0);
                circle(12.0, 12.0, 2.0);
                for index in 0..8 {
                    let angle = index as f32 * std::f32::consts::TAU / 8.0;
                    line(
                        (12.0 + angle.cos() * 8.0, 12.0 + angle.sin() * 8.0),
                        (12.0 + angle.cos() * 10.0, 12.0 + angle.sin() * 10.0),
                    );
                }
            }
            Self::File => {
                path(&[
                    (5.0, 2.0),
                    (14.0, 2.0),
                    (20.0, 8.0),
                    (20.0, 22.0),
                    (5.0, 22.0),
                    (5.0, 2.0),
                ]);
                path(&[(14.0, 2.0), (14.0, 8.0), (20.0, 8.0)]);
                line((9.0, 13.0), (16.0, 13.0));
                line((9.0, 17.0), (16.0, 17.0));
            }
            Self::Folder => path(&[
                (2.0, 6.0),
                (9.0, 6.0),
                (11.0, 9.0),
                (22.0, 9.0),
                (20.0, 20.0),
                (2.0, 20.0),
                (2.0, 6.0),
            ]),
            Self::Inbox => {
                path(&[
                    (3.0, 7.0),
                    (21.0, 7.0),
                    (23.0, 18.0),
                    (23.0, 20.0),
                    (1.0, 20.0),
                    (1.0, 18.0),
                    (3.0, 7.0),
                ]);
                path(&[
                    (2.0, 14.0),
                    (8.0, 14.0),
                    (10.0, 17.0),
                    (14.0, 17.0),
                    (16.0, 14.0),
                    (22.0, 14.0),
                ]);
            }
            Self::Star => {
                let points: Vec<_> = (0..10)
                    .map(|index| {
                        let angle = index as f32 * std::f32::consts::TAU / 10.0
                            - std::f32::consts::FRAC_PI_2;
                        let radius = if index % 2 == 0 { 10.0 } else { 4.5 };
                        at(12.0 + angle.cos() * radius, 12.0 + angle.sin() * radius)
                    })
                    .collect();
                painter.add(egui::Shape::closed_line(points, stroke));
            }
            Self::Trash => {
                line((3.0, 6.0), (21.0, 6.0));
                path(&[(9.0, 6.0), (9.0, 3.0), (15.0, 3.0), (15.0, 6.0)]);
                path(&[(5.0, 6.0), (7.0, 21.0), (17.0, 21.0), (19.0, 6.0)]);
                line((10.0, 10.0), (10.0, 17.0));
                line((14.0, 10.0), (14.0, 17.0));
            }
            Self::Tag => {
                path(&[
                    (3.0, 3.0),
                    (12.0, 3.0),
                    (22.0, 13.0),
                    (13.0, 22.0),
                    (3.0, 12.0),
                    (3.0, 3.0),
                ]);
                circle(7.0, 7.0, 1.0);
            }
            Self::Search => {
                circle(10.0, 10.0, 7.0);
                line((15.0, 15.0), (22.0, 22.0));
            }
            Self::Check => path(&[(3.0, 12.0), (9.0, 18.0), (21.0, 5.0)]),
            Self::Close => {
                line((5.0, 5.0), (19.0, 19.0));
                line((19.0, 5.0), (5.0, 19.0));
            }
            Self::Info => {
                circle(12.0, 12.0, 10.0);
                painter.circle_filled(at(12.0, 7.0), bounds.width() / 16.0, color);
                line((12.0, 11.0), (12.0, 17.0));
            }
            Self::Sun => {
                circle(12.0, 12.0, 4.0);
                for index in 0..8 {
                    let angle = index as f32 * std::f32::consts::TAU / 8.0;
                    line(
                        (12.0 + angle.cos() * 7.0, 12.0 + angle.sin() * 7.0),
                        (12.0 + angle.cos() * 10.5, 12.0 + angle.sin() * 10.5),
                    );
                }
            }
            Self::Moon => {
                curve([(19.0, 17.0), (8.0, 21.0), (2.0, 8.0), (13.0, 3.0)]);
                curve([(13.0, 3.0), (5.0, 14.0), (17.0, 21.0), (21.0, 14.0)]);
            }
            Self::Cube => {
                path(&[
                    (12.0, 2.0),
                    (22.0, 7.0),
                    (22.0, 17.0),
                    (12.0, 22.0),
                    (2.0, 17.0),
                    (2.0, 7.0),
                    (12.0, 2.0),
                ]);
                path(&[(2.0, 7.0), (12.0, 12.0), (22.0, 7.0)]);
                line((12.0, 12.0), (12.0, 22.0));
            }
        }
    }
}

/// Draw an icon centered within a larger clickable/control rectangle.
pub fn centered(icon: Icon, painter: &Painter, bounds: Rect, color: Color32) {
    icon.draw(
        painter,
        Rect::from_center_size(bounds.center(), egui::vec2(16.0, 16.0)),
        color,
    );
}
