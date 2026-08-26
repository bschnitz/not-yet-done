//! The style grammar shared by highlight rules and the `load`-hook script
//! channel — see `docs/plan-row-highlighting.md`.
//!
//! A *style* says how something is painted; *where* it is painted is decided
//! by the rule (or the script) that carries it. Three surface forms are
//! accepted, mirroring [`TabUnreadStyle`](super::view_config::TabUnreadStyle)
//! so the config surface stays one idiom:
//!
//! ```yaml
//! style: over-budget                       # a name from `styles:` (or a theme role)
//! style: [bold]                            # font modifiers only
//! style: { bg: "#7a1c1c", fg: auto }       # the full form, every field optional
//! ```
//!
//! Unlike the rest of the config, colours here are primarily literal
//! `#rrggbb`: a highlight encodes a *domain* meaning ("over budget") the theme
//! has no name for, and a computed ramp needs values no palette can hold.
//! Theme role names stay accepted as a convenience.
//!
//! Nothing paints from here yet: the render paths grow their highlight
//! precedence in a later phase of the plan. Drop the `dead_code` allowance
//! below once they do.
#![allow(dead_code)]

use std::collections::HashMap;
use std::str::FromStr;

use ratatui::style::{Color, Modifier};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::color::HexColor;
use super::view_config::TextModifier;
use crate::ui::theme::Theme;

// ---------------------------------------------------------------------------
// Colours
// ---------------------------------------------------------------------------

/// One colour slot of a style: a literal hex value, a theme role name, or
/// `auto` (foreground only — pick whatever contrasts with the background that
/// ends up beneath this text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColorSpec {
    /// `"#rrggbb"` — the primary form here.
    Hex(HexColor),
    /// A theme role name (`accent`, `error`, …), resolved via [`Theme::role`].
    Role(String),
    /// `auto` — decided at paint time from the effective background.
    Auto,
}

impl ColorSpec {
    /// The fixed colour this spec names, or `None` for [`ColorSpec::Auto`] and
    /// for a role the theme does not know.
    pub fn fixed(&self, theme: &Theme) -> Option<Color> {
        match self {
            Self::Hex(c) => Some(c.to_ratatui()),
            Self::Role(name) => theme.role(name),
            Self::Auto => None,
        }
    }

    fn as_yaml(&self) -> String {
        match self {
            Self::Hex(c) => c.to_string(),
            Self::Role(name) => name.clone(),
            Self::Auto => "auto".to_string(),
        }
    }
}

impl FromStr for ColorSpec {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.starts_with('#') {
            // Only the `#` form is parsed as hex: a bare `abc123` is far more
            // likely a misspelt role name than a colour, and letting it through
            // as one would swallow the validator's warning.
            return HexColor::from_str(trimmed).map(Self::Hex);
        }
        if trimmed.eq_ignore_ascii_case("auto") {
            return Ok(Self::Auto);
        }
        if trimmed.is_empty() {
            return Err("empty colour".to_string());
        }
        Ok(Self::Role(trimmed.to_string()))
    }
}

impl<'de> Deserialize<'de> for ColorSpec {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        ColorSpec::from_str(&raw).map_err(serde::de::Error::custom)
    }
}

impl Serialize for ColorSpec {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.as_yaml())
    }
}

// ---------------------------------------------------------------------------
// Modes
// ---------------------------------------------------------------------------

/// A render surface a highlight can be restricted to.
///
/// Writable in two places, meaning different things: on a *style* it says the
/// colour is meant for those surfaces, on a *rule* it says the rule only fires
/// there. The rule wins, replacing the style's list outright — see
/// [`ResolvedStyle::with_modes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HighlightMode {
    /// The normal table.
    Table,
    /// Card mode (`card:`).
    Card,
    /// The record-detail split (`o`).
    Details,
    /// The label line in tree mode.
    Tree,
}

// ---------------------------------------------------------------------------
// The style surface
// ---------------------------------------------------------------------------

/// A style as written in YAML or emitted by a script.
///
/// Deserialized by hand rather than with `#[serde(untagged)]`: untagged
/// reports nothing but "data did not match any variant" for a misspelt field,
/// which is exactly the invisible-config-bug class the view parser already
/// fights. Dispatching on the value's shape lets the inline form's own
/// `deny_unknown_fields` error through by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum StyleSpec {
    /// A name: an entry of a `styles:` map, or — failing that — a theme role
    /// used as the foreground.
    Name(String),
    /// A bare modifier list (`[bold]`) — font change only.
    Modifiers(Vec<TextModifier>),
    /// The full form; every field optional.
    Inline(Box<InlineStyle>),
}

impl<'de> Deserialize<'de> for StyleSpec {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct SpecVisitor;

        impl<'de> serde::de::Visitor<'de> for SpecVisitor {
            type Value = StyleSpec;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a style name, a list of modifiers, or a style mapping")
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<StyleSpec, E> {
                Ok(StyleSpec::Name(v.to_string()))
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                seq: A,
            ) -> Result<StyleSpec, A::Error> {
                Vec::<TextModifier>::deserialize(serde::de::value::SeqAccessDeserializer::new(seq))
                    .map(StyleSpec::Modifiers)
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                map: A,
            ) -> Result<StyleSpec, A::Error> {
                InlineStyle::deserialize(serde::de::value::MapAccessDeserializer::new(map))
                    .map(|i| StyleSpec::Inline(Box::new(i)))
            }
        }

        d.deserialize_any(SpecVisitor)
    }
}

/// The mapping form of [`StyleSpec`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InlineStyle {
    /// Foreground. `auto` picks a contrasting colour at paint time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fg: Option<ColorSpec>,
    /// Background. `auto` is meaningless here and is reported by the
    /// validator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg: Option<ColorSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modifiers: Vec<TextModifier>,
    /// What to paint instead while the row is the cursor row. Nested styles
    /// carry no `selected:` and no `modes:` of their own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<Box<StyleSpec>>,
    /// Where this style applies. `None` = every surface it can apply to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modes: Option<Vec<HighlightMode>>,
}

impl StyleSpec {
    /// Config problems that do not stop resolution but should be reported —
    /// see the unknown-view-field warning for the mechanism. `path` is the
    /// location shown to the user (e.g. `views[2].highlights[0].style`).
    pub fn warnings(&self, path: &str, out: &mut Vec<String>) {
        self.collect_warnings(path, false, out);
    }

    fn collect_warnings(&self, path: &str, nested: bool, out: &mut Vec<String>) {
        let Self::Inline(inline) = self else {
            return;
        };
        if matches!(inline.bg, Some(ColorSpec::Auto)) {
            out.push(format!(
                "{path}: `bg: auto` has nothing to contrast against and is ignored \
                 (`auto` is a foreground)"
            ));
        }
        if nested {
            if inline.modes.is_some() {
                out.push(format!(
                    "{path}: `modes:` inside `selected:` is ignored — the selected \
                     state is not a render surface"
                ));
            }
            if inline.selected.is_some() {
                out.push(format!(
                    "{path}: `selected:` inside `selected:` is ignored — the \
                     selected state does not nest"
                ));
            }
        }
        if let Some(sel) = &inline.selected {
            sel.collect_warnings(&format!("{path}.selected"), true, out);
        }
    }
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

/// One paintable layer: whatever it does *not* set is left to the layer below.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StyleLayer {
    pub fg: Option<FgSpec>,
    pub bg: Option<Color>,
    pub modifiers: Modifier,
}

/// The foreground of a resolved layer. `Auto` survives resolution because the
/// background it has to contrast against is only known once every layer below
/// has been applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FgSpec {
    Fixed(Color),
    Auto,
}

impl StyleLayer {
    /// Lay `top` over `self`, field by field: a field `top` leaves unset keeps
    /// this layer's value. Modifiers accumulate rather than replace — two
    /// rules asking for bold and italic mean both.
    pub fn layer(self, top: StyleLayer) -> StyleLayer {
        StyleLayer {
            fg: top.fg.or(self.fg),
            bg: top.bg.or(self.bg),
            modifiers: self.modifiers | top.modifiers,
        }
    }

    /// Turn this layer into a ratatui style, resolving an `auto` foreground
    /// against `bg_below` — the background actually in force at this point,
    /// which is this layer's own `bg` when it sets one.
    pub fn to_ratatui(self, theme: &Theme, bg_below: Option<Color>) -> ratatui::style::Style {
        let mut style = ratatui::style::Style::default().add_modifier(self.modifiers);
        if let Some(bg) = self.bg {
            style = style.bg(bg);
        }
        let effective_bg = self.bg.or(bg_below);
        match self.fg {
            Some(FgSpec::Fixed(c)) => style = style.fg(c),
            // No background anywhere → nothing to contrast against, so the
            // layer below keeps its foreground. Resolving against the theme
            // background instead would silently throw a column colour away.
            Some(FgSpec::Auto) => {
                if let Some(fg) = effective_bg.and_then(|bg| auto_fg(theme, bg)) {
                    style = style.fg(fg);
                }
            }
            None => {}
        }
        style
    }
}

/// A style resolved against the theme: two layers plus its reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedStyle {
    /// The layer for an ordinary row.
    pub normal: StyleLayer,
    /// What `selected:` set explicitly, if anything. Use
    /// [`Self::selected_layer`] rather than this field.
    pub selected: Option<StyleLayer>,
    /// `None` = every surface.
    pub modes: Option<Vec<HighlightMode>>,
}

impl ResolvedStyle {
    /// The layer to paint with while the row is the cursor row.
    ///
    /// The normal layer's foreground and modifiers carry over — a highlight
    /// that vanishes under the cursor is worse than none — but its
    /// **background deliberately does not**: without an explicit
    /// `selected.bg`, the table's `RowSelected` background stays, so the
    /// cursor remains visible on exactly the rows that were made conspicuous.
    pub fn selected_layer(&self) -> StyleLayer {
        let carried = StyleLayer {
            bg: None,
            ..self.normal
        };
        match self.selected {
            Some(sel) => carried.layer(sel),
            None => carried,
        }
    }

    /// Whether this style paints on `mode`.
    pub fn applies_to(&self, mode: HighlightMode) -> bool {
        match &self.modes {
            Some(list) => list.contains(&mode),
            None => true,
        }
    }

    /// Replace the reach with the one the *rule* declares. The rule has the
    /// last word: a shared style's `modes:` is only the default it brings
    /// along. Intersecting instead would let a style saying `[table]` and a
    /// rule saying `[card]` paint nothing at all, with nothing to report.
    pub fn with_modes(mut self, modes: Option<Vec<HighlightMode>>) -> Self {
        if modes.is_some() {
            self.modes = modes;
        }
        self
    }
}

/// Why a style could not be resolved. Both cases are config errors the
/// validator reports; the render path skips the highlight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StyleError {
    /// A name that is neither in a `styles:` map nor a theme role.
    UnknownName(String),
    /// `styles:` entries referring to each other in a circle.
    Cycle(String),
}

impl std::fmt::Display for StyleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownName(n) => write!(
                f,
                "unknown style {n:?} — not in `styles:` and not a theme colour"
            ),
            Self::Cycle(n) => write!(f, "style {n:?} refers to itself in a circle"),
        }
    }
}

/// Named style tables plus the theme, in the order a name is looked up:
/// the view's own `styles:`, then the theme's, then a theme role name.
pub struct StyleResolver<'a> {
    view_styles: &'a HashMap<String, StyleSpec>,
    theme_styles: &'a HashMap<String, StyleSpec>,
    theme: &'a Theme,
}

/// A `styles:` map that never has entries, for call sites without a view file.
static NO_STYLES: std::sync::LazyLock<HashMap<String, StyleSpec>> =
    std::sync::LazyLock::new(HashMap::new);

impl<'a> StyleResolver<'a> {
    pub fn new(
        view_styles: &'a HashMap<String, StyleSpec>,
        theme_styles: &'a HashMap<String, StyleSpec>,
        theme: &'a Theme,
    ) -> Self {
        Self {
            view_styles,
            theme_styles,
            theme,
        }
    }

    /// A resolver with only the theme's `styles:` — for scripts and other
    /// callers that are not inside a view file.
    pub fn theme_only(theme_styles: &'a HashMap<String, StyleSpec>, theme: &'a Theme) -> Self {
        Self::new(&NO_STYLES, theme_styles, theme)
    }

    pub fn resolve(&self, spec: &StyleSpec) -> Result<ResolvedStyle, StyleError> {
        self.resolve_inner(spec, &mut Vec::new())
    }

    fn resolve_inner(
        &self,
        spec: &StyleSpec,
        seen: &mut Vec<String>,
    ) -> Result<ResolvedStyle, StyleError> {
        match spec {
            StyleSpec::Name(name) => self.resolve_name(name, seen),
            StyleSpec::Modifiers(mods) => Ok(ResolvedStyle {
                normal: StyleLayer {
                    modifiers: fold_modifiers(mods),
                    ..StyleLayer::default()
                },
                selected: None,
                modes: None,
            }),
            StyleSpec::Inline(inline) => self.resolve_inline(inline, seen),
        }
    }

    fn resolve_name(
        &self,
        name: &str,
        seen: &mut Vec<String>,
    ) -> Result<ResolvedStyle, StyleError> {
        if seen.iter().any(|s| s == name) {
            return Err(StyleError::Cycle(name.to_string()));
        }
        if let Some(spec) = self
            .view_styles
            .get(name)
            .or_else(|| self.theme_styles.get(name))
        {
            seen.push(name.to_string());
            let resolved = self.resolve_inner(spec, seen);
            seen.pop();
            return resolved;
        }
        // Not a named style — fall through to a theme role used as foreground,
        // so `style: accent` keeps working without a `styles:` entry.
        match self.theme.role(name) {
            Some(color) => Ok(ResolvedStyle {
                normal: StyleLayer {
                    fg: Some(FgSpec::Fixed(color)),
                    ..StyleLayer::default()
                },
                selected: None,
                modes: None,
            }),
            None => Err(StyleError::UnknownName(name.to_string())),
        }
    }

    fn resolve_inline(
        &self,
        inline: &InlineStyle,
        seen: &mut Vec<String>,
    ) -> Result<ResolvedStyle, StyleError> {
        let mut base = ResolvedStyle {
            normal: StyleLayer::default(),
            selected: None,
            modes: None,
        };

        let own = StyleLayer {
            fg: inline.fg.as_ref().map(|c| self.fg_spec(c)),
            bg: inline.bg.as_ref().and_then(|c| c.fixed(self.theme)),
            modifiers: fold_modifiers(&inline.modifiers),
        };
        base.normal = base.normal.layer(own);

        if let Some(sel) = &inline.selected {
            // A nested style contributes only its own layer: its `selected:`
            // and `modes:` are meaningless here and warned about separately.
            let sel_layer = self.resolve_inner(sel, seen)?.normal;
            base.selected = Some(match base.selected {
                Some(below) => below.layer(sel_layer),
                None => sel_layer,
            });
        }
        if inline.modes.is_some() {
            base.modes = inline.modes.clone();
        }
        Ok(base)
    }

    fn fg_spec(&self, c: &ColorSpec) -> FgSpec {
        match c {
            ColorSpec::Auto => FgSpec::Auto,
            other => match other.fixed(self.theme) {
                Some(color) => FgSpec::Fixed(color),
                // An unknown role in a colour slot: the validator reports it,
                // the paint path leaves the foreground alone.
                None => FgSpec::Fixed(self.theme.text_med()),
            },
        }
    }
}

fn fold_modifiers(list: &[TextModifier]) -> Modifier {
    list.iter()
        .fold(Modifier::empty(), |acc, m| acc | m.to_ratatui())
}

// ---------------------------------------------------------------------------
// `fg: auto`
// ---------------------------------------------------------------------------

/// Pick the theme's light or dark auto-foreground, whichever contrasts better
/// with `bg`.
///
/// Scored by WCAG relative-luminance contrast ratio rather than a fixed
/// lightness threshold: the two candidates are configurable, and a threshold
/// would be wrong the moment they are not black and white.
pub fn auto_fg(theme: &Theme, bg: Color) -> Option<Color> {
    let bg_lum = luminance(bg)?;
    let light = theme.auto_fg_light();
    let dark = theme.auto_fg_dark();
    let light_ratio = luminance(light).map(|l| contrast(bg_lum, l));
    let dark_ratio = luminance(dark).map(|l| contrast(bg_lum, l));
    match (light_ratio, dark_ratio) {
        (Some(a), Some(b)) => Some(if a >= b { light } else { dark }),
        (Some(_), None) => Some(light),
        (None, Some(_)) => Some(dark),
        (None, None) => None,
    }
}

/// WCAG relative luminance. `None` for anything that is not a 24-bit colour —
/// a terminal-palette index has no known RGB value to measure.
fn luminance(color: Color) -> Option<f32> {
    let Color::Rgb(r, g, b) = color else {
        return None;
    };
    fn channel(v: u8) -> f32 {
        let c = v as f32 / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    Some(0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b))
}

/// WCAG contrast ratio between two relative luminances (1.0 … 21.0).
fn contrast(a: f32, b: f32) -> f32 {
    let (hi, lo) = if a >= b { (a, b) } else { (b, a) };
    (hi + 0.05) / (lo + 0.05)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ThemeConfig;

    fn theme() -> Theme {
        Theme::new(ThemeConfig::default())
    }

    fn parse(yaml: &str) -> StyleSpec {
        serde_yaml::from_str(yaml).expect("style should parse")
    }

    fn named(entries: &[(&str, &str)]) -> HashMap<String, StyleSpec> {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), parse(v)))
            .collect()
    }

    fn rgb(hex: &str) -> Color {
        HexColor::from_str(hex).unwrap().to_ratatui()
    }

    // ── The three surface forms ──────────────────────────────────────────

    #[test]
    fn all_three_style_forms_parse() {
        assert_eq!(parse("over-budget"), StyleSpec::Name("over-budget".into()));
        assert_eq!(
            parse("[bold, italic]"),
            StyleSpec::Modifiers(vec![TextModifier::Bold, TextModifier::Italic])
        );
        let StyleSpec::Inline(inline) = parse(r#"{ bg: '#7a1c1c', fg: auto }"#) else {
            panic!("mapping form should parse as Inline");
        };
        assert_eq!(inline.bg, Some(ColorSpec::Hex(HexColor(0x7a, 0x1c, 0x1c))));
        assert_eq!(inline.fg, Some(ColorSpec::Auto));
    }

    #[test]
    fn colour_slot_tells_hex_auto_and_role_apart() {
        assert_eq!(
            ColorSpec::from_str("#010203"),
            Ok(ColorSpec::Hex(HexColor(1, 2, 3)))
        );
        assert_eq!(ColorSpec::from_str("auto"), Ok(ColorSpec::Auto));
        assert_eq!(
            ColorSpec::from_str("accent"),
            Ok(ColorSpec::Role("accent".into()))
        );
        // A bare six-digit word is a role name, not a colour: without the `#`
        // it is far more likely a typo, and the validator should say so.
        assert_eq!(
            ColorSpec::from_str("abcdef"),
            Ok(ColorSpec::Role("abcdef".into()))
        );
        assert!(ColorSpec::from_str("#nothex").is_err());
    }

    #[test]
    fn the_same_grammar_parses_out_of_a_scripts_json() {
        // The `load` hook answers in JSON, not YAML — one grammar, two
        // formats, so the visitor has to dispatch on shape rather than syntax.
        let spec: StyleSpec =
            serde_json::from_str(r##"{"bg": "#7a1c1c", "fg": "auto", "modes": ["table"]}"##)
                .unwrap();
        let theme = theme();
        let empty = HashMap::new();
        let resolved = StyleResolver::theme_only(&empty, &theme)
            .resolve(&spec)
            .unwrap();
        assert_eq!(resolved.normal.bg, Some(rgb("#7a1c1c")));
        assert_eq!(resolved.normal.fg, Some(FgSpec::Auto));
        assert!(!resolved.applies_to(HighlightMode::Card));

        let named: StyleSpec = serde_json::from_str(r#""over-budget""#).unwrap();
        assert_eq!(named, StyleSpec::Name("over-budget".into()));
    }

    #[test]
    fn unknown_inline_field_is_rejected() {
        let err = serde_yaml::from_str::<StyleSpec>(r#"{ background: '#000000' }"#).unwrap_err();
        assert!(err.to_string().contains("background"), "{err}");
    }

    // ── Name resolution ──────────────────────────────────────────────────

    #[test]
    fn view_styles_shadow_theme_styles() {
        let theme = theme();
        let view = named(&[("hot", r#"{ bg: '#111111' }"#)]);
        let global = named(&[("hot", r#"{ bg: '#222222' }"#)]);
        let resolved = StyleResolver::new(&view, &global, &theme)
            .resolve(&StyleSpec::Name("hot".into()))
            .unwrap();
        assert_eq!(resolved.normal.bg, Some(rgb("#111111")));
    }

    #[test]
    fn a_name_falls_through_to_a_theme_role() {
        let theme = theme();
        let empty = HashMap::new();
        let resolved = StyleResolver::new(&empty, &empty, &theme)
            .resolve(&StyleSpec::Name("accent".into()))
            .unwrap();
        assert_eq!(resolved.normal.fg, Some(FgSpec::Fixed(theme.accent())));
        assert_eq!(resolved.normal.bg, None);
    }

    #[test]
    fn an_unknown_name_is_an_error_not_a_silent_default() {
        let theme = theme();
        let empty = HashMap::new();
        let err = StyleResolver::new(&empty, &empty, &theme)
            .resolve(&StyleSpec::Name("nonesuch".into()))
            .unwrap_err();
        assert_eq!(err, StyleError::UnknownName("nonesuch".into()));
    }

    #[test]
    fn styles_referring_to_each_other_in_a_circle_do_not_hang() {
        let theme = theme();
        let styles = named(&[("a", "b"), ("b", "a")]);
        let empty = HashMap::new();
        let err = StyleResolver::new(&styles, &empty, &theme)
            .resolve(&StyleSpec::Name("a".into()))
            .unwrap_err();
        assert!(matches!(err, StyleError::Cycle(_)), "{err:?}");
    }

    // ── fg: auto ─────────────────────────────────────────────────────────

    #[test]
    fn auto_fg_picks_the_candidate_with_the_better_contrast() {
        let theme = theme();
        assert_eq!(auto_fg(&theme, rgb("#ffffff")), Some(theme.auto_fg_dark()));
        assert_eq!(auto_fg(&theme, rgb("#000000")), Some(theme.auto_fg_light()));
        // A deep red ramp end: dark text on it would be unreadable.
        assert_eq!(auto_fg(&theme, rgb("#7a1c1c")), Some(theme.auto_fg_light()));
    }

    #[test]
    fn auto_fg_resolves_against_the_styles_own_background() {
        let theme = theme();
        let empty = HashMap::new();
        let resolved = StyleResolver::new(&empty, &empty, &theme)
            .resolve(&parse(r#"{ bg: '#ffffff', fg: auto }"#))
            .unwrap();
        let style = resolved.normal.to_ratatui(&theme, None);
        assert_eq!(style.fg, Some(theme.auto_fg_dark()));
        assert_eq!(style.bg, Some(rgb("#ffffff")));
    }

    #[test]
    fn auto_fg_falls_through_when_no_background_is_in_force() {
        let theme = theme();
        let empty = HashMap::new();
        let resolved = StyleResolver::new(&empty, &empty, &theme)
            .resolve(&parse("{ fg: auto }"))
            .unwrap();
        // Nothing to contrast against — the layer below keeps its foreground
        // rather than the column colour being thrown away silently.
        assert_eq!(resolved.normal.to_ratatui(&theme, None).fg, None);
        // A background from further down the stack does count.
        assert_eq!(
            resolved.normal.to_ratatui(&theme, Some(rgb("#ffffff"))).fg,
            Some(theme.auto_fg_dark())
        );
    }

    // ── The selected state ───────────────────────────────────────────────

    #[test]
    fn the_cursor_row_keeps_the_foreground_but_not_the_background() {
        let theme = theme();
        let empty = HashMap::new();
        let resolved = StyleResolver::new(&empty, &empty, &theme)
            .resolve(&parse(
                r#"{ fg: '#ff0000', bg: '#330000', modifiers: [bold] }"#,
            ))
            .unwrap();
        let sel = resolved.selected_layer();
        assert_eq!(sel.fg, Some(FgSpec::Fixed(rgb("#ff0000"))));
        // Dropping the background is the point: without it the table's
        // RowSelected fill stays and the cursor is still findable.
        assert_eq!(sel.bg, None);
        assert!(sel.modifiers.contains(Modifier::BOLD));
    }

    #[test]
    fn an_explicit_selected_background_wins() {
        let theme = theme();
        let empty = HashMap::new();
        let resolved = StyleResolver::new(&empty, &empty, &theme)
            .resolve(&parse(r#"{ bg: '#330000', selected: { bg: '#9c2a2a' } }"#))
            .unwrap();
        assert_eq!(resolved.selected_layer().bg, Some(rgb("#9c2a2a")));
    }

    #[test]
    fn auto_fg_is_recomputed_for_the_selected_background() {
        let theme = theme();
        let empty = HashMap::new();
        let resolved = StyleResolver::new(&empty, &empty, &theme)
            .resolve(&parse(
                r#"{ bg: '#000000', fg: auto, selected: { bg: '#ffffff' } }"#,
            ))
            .unwrap();
        assert_eq!(
            resolved.normal.to_ratatui(&theme, None).fg,
            Some(theme.auto_fg_light())
        );
        // Same style, other background — contrast must not collapse on
        // exactly the row the user is looking at.
        assert_eq!(
            resolved.selected_layer().to_ratatui(&theme, None).fg,
            Some(theme.auto_fg_dark())
        );
    }

    // ── Modes ────────────────────────────────────────────────────────────

    #[test]
    fn a_style_without_modes_paints_everywhere() {
        let theme = theme();
        let empty = HashMap::new();
        let resolved = StyleResolver::new(&empty, &empty, &theme)
            .resolve(&parse(r#"{ bg: '#111111' }"#))
            .unwrap();
        assert!(resolved.applies_to(HighlightMode::Table));
        assert!(resolved.applies_to(HighlightMode::Card));
        assert!(resolved.applies_to(HighlightMode::Details));
        assert!(resolved.applies_to(HighlightMode::Tree));
    }

    #[test]
    fn the_rules_modes_replace_the_styles_rather_than_intersecting() {
        let theme = theme();
        let empty = HashMap::new();
        let resolved = StyleResolver::new(&empty, &empty, &theme)
            .resolve(&parse(r#"{ bg: '#111111', modes: [table] }"#))
            .unwrap();
        assert!(!resolved.applies_to(HighlightMode::Card));
        // Same shared style, used by a rule that reaches elsewhere. An
        // intersection would paint nothing at all here, with nothing to report.
        let on_rule = resolved.with_modes(Some(vec![HighlightMode::Card]));
        assert!(on_rule.applies_to(HighlightMode::Card));
        assert!(!on_rule.applies_to(HighlightMode::Table));
    }

    #[test]
    fn a_rule_without_modes_keeps_the_styles_reach() {
        let theme = theme();
        let empty = HashMap::new();
        let resolved = StyleResolver::new(&empty, &empty, &theme)
            .resolve(&parse(r#"{ bg: '#111111', modes: [table] }"#))
            .unwrap()
            .with_modes(None);
        assert!(resolved.applies_to(HighlightMode::Table));
        assert!(!resolved.applies_to(HighlightMode::Card));
    }

    // ── Layering ─────────────────────────────────────────────────────────

    #[test]
    fn later_layers_win_per_field_and_modifiers_accumulate() {
        let below = StyleLayer {
            fg: Some(FgSpec::Fixed(rgb("#ff0000"))),
            bg: Some(rgb("#111111")),
            modifiers: Modifier::BOLD,
        };
        let above = StyleLayer {
            fg: None,
            bg: Some(rgb("#222222")),
            modifiers: Modifier::ITALIC,
        };
        let merged = below.layer(above);
        // Unset above → the layer below keeps its value.
        assert_eq!(merged.fg, Some(FgSpec::Fixed(rgb("#ff0000"))));
        assert_eq!(merged.bg, Some(rgb("#222222")));
        assert_eq!(merged.modifiers, Modifier::BOLD | Modifier::ITALIC);
    }

    // ── Warnings ─────────────────────────────────────────────────────────

    #[test]
    fn bg_auto_is_reported() {
        let mut out = Vec::new();
        parse("{ bg: auto }").warnings("views[0].highlights[0].style", &mut out);
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].contains("bg: auto"), "{out:?}");
    }

    #[test]
    fn modes_inside_selected_is_reported() {
        let mut out = Vec::new();
        parse(r#"{ bg: '#111111', selected: { bg: '#222222', modes: [card] } }"#)
            .warnings("style", &mut out);
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].contains("modes"), "{out:?}");
        assert!(out[0].contains("selected"), "{out:?}");
    }

    #[test]
    fn a_clean_style_produces_no_warnings() {
        let mut out = Vec::new();
        parse(r#"{ bg: '#7a1c1c', fg: auto, modes: [table], selected: { bg: '#9c2a2a' } }"#)
            .warnings("style", &mut out);
        assert!(out.is_empty(), "{out:?}");
    }
}
