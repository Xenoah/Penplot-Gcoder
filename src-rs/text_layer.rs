//! Editable text metadata. Stored strokes remain authoritative on project load.
use crate::{
    Point,
    text_outline::{FontIdentity, MAX_TEXT_CHARS, MAX_TEXT_LINES},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextLayer {
    pub content: String,
    pub font: FontIdentity,
    pub em_mm: f64,
    pub tolerance_mm: f64,
    /// [a,b,c,d,x,y]: maps normalized text mm into the document's source coordinates.
    pub transform: [f64; 6],
}

impl TextLayer {
    pub fn map(&self, p: Point) -> Point {
        let [a, b, c, d, x, y] = self.transform;
        Point::new(a * p.x + c * p.y + x, b * p.x + d * p.y + y)
    }
    pub fn validate(&self) -> Result<(), String> {
        let text = self
            .content
            .replace("\r\n", "\n")
            .replace(['\r', '\u{2028}', '\u{2029}'], "\n");
        if text.trim().is_empty()
            || self.content.len() > MAX_TEXT_CHARS * 4
            || self.content.chars().count() > MAX_TEXT_CHARS
            || text.lines().count() > MAX_TEXT_LINES
            || text
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t')
        {
            return Err("Invalid text: enter up to 2,000 Unicode characters and 100 lines.".into());
        }
        if !(0.1..=500.).contains(&self.em_mm) || !(0.001..=1.).contains(&self.tolerance_mm) {
            return Err("Invalid text size or outline tolerance.".into());
        }
        let [a, b, c, d, _, _] = self.transform;
        let det = a * d - b * c;
        let norm = (a * a + b * b).max(c * c + d * d);
        if (a * c + b * d).abs() > norm * 1e-8
            || ((a * a + b * b) - (c * c + d * d)).abs() > norm * 1e-8
            || det <= 0.
        {
            return Err("Text placement must use a positive uniform scale and rotation.".into());
        }
        if !self.transform.iter().all(|v| (-1e8..=1e8).contains(v))
            || !det.is_finite()
            || det.abs() < 1e-16
        {
            return Err("Invalid text placement.".into());
        }
        let valid_name =
            |s: &str| !s.trim().is_empty() && s.len() <= 256 && !s.chars().any(char::is_control);
        if self.font.face_index >= 256
            || !valid_name(&self.font.family)
            || !valid_name(&self.font.style)
            || self
                .font
                .postscript_name
                .as_deref()
                .is_some_and(|s| !valid_name(s))
            || self.font.sha256.len() != 64
            || !self
                .font
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("Invalid text font identity.".into());
        }
        Ok(())
    }
}
