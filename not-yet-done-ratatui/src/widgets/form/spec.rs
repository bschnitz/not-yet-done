/// The kind of a single form field, driving which widget backs it.
///
/// App-agnostic: this spec knows nothing about any application's field model.
/// Consumers map their own field descriptions onto it.
#[derive(Debug, Clone)]
pub enum FormFieldKind {
    /// Free-text single-line input.
    Text,
    /// Single-choice dropdown over `options`.
    Select { options: Vec<String> },
    /// Boolean on/off toggle.
    Toggle,
    /// Natural-language date/time text field. With the `natural-date` feature a
    /// live resolved-preview line is shown below the input. The submitted value
    /// is always the raw phrase, so the consumer keeps control of resolution.
    DateTime { with_time: bool },
    /// Whole number. `min`/`max` are checked on submit rather than while
    /// typing — a range that rejects every prefix of the number the user is
    /// about to enter would make the field unusable. `step` is what the
    /// up/down keys add and subtract (`None` → one).
    Number {
        min: Option<i64>,
        max: Option<i64>,
        step: Option<i64>,
    },
    /// Fractional number. `precision` caps the digits after the separator and
    /// *is* enforced while typing, because every prefix of a too-precise number
    /// is already too precise. Which separator the user types is a display
    /// setting ([`FormOptions::decimal_separator`](super::FormOptions)); the
    /// submitted value always uses a dot, so no consumer depends on it.
    Decimal {
        min: Option<f64>,
        max: Option<f64>,
        precision: Option<u8>,
    },
    /// A span of time. The field takes the usual spellings (`1:30`, `90m`,
    /// `1h30`, `1.5h`), shows what it made of them, and submits **whole
    /// minutes** as a decimal string — unlike a date there is no ambiguity
    /// worth deferring to the consumer.
    Duration,
    /// Any number of choices out of `options`, submitted newline-joined in
    /// option order.
    MultiSelect { options: Vec<String> },
    /// Free text over several lines, submitted with the newlines in it.
    /// `lines` is how many rows the box is given; the text scrolls beyond
    /// that. It is a field to fill in, not an editing session — that is what
    /// opening an editor is for.
    Multiline { lines: Option<u16> },
}

/// A predicate over another field's current value, gating a field's visibility.
///
/// The form re-evaluates this after every value change: a field whose condition
/// no longer holds is hidden — dropped from the layout, skipped by focus
/// navigation, and excluded from the submitted values and required-checks.
/// Comparison is against the controller's *current value* string (for a toggle,
/// `"true"`/`"false"`; for a select, the option label).
#[derive(Debug, Clone)]
pub struct FieldCondition {
    /// Key of the field whose value is tested.
    pub field: String,
    /// The field is visible when the controller's value equals one of these
    /// (an empty list matches nothing, so the field is always hidden — useful
    /// only with `negate`).
    pub equals_any: Vec<String>,
    /// Invert the match: visible when the value is *not* among `equals_any`.
    pub negate: bool,
}

/// Declarative description of one form field.
#[derive(Debug, Clone)]
pub struct FormFieldSpec {
    /// Stable key under which the value is returned in [`super::Form::values`].
    pub key: String,
    /// Human-readable label rendered as the field title.
    pub label: String,
    /// The field kind (and its backing widget).
    pub kind: FormFieldKind,
    /// When `true`, submit is blocked while the value is empty.
    pub required: bool,
    /// Optional initial value (overridden by an explicit prefill entry).
    pub default: Option<String>,
    /// Render the value as bullets instead of clear text. Only
    /// [`FormFieldKind::Text`] honours it — a select shows option labels and a
    /// toggle a state, neither of which is a secret worth hiding.
    pub masked: bool,
    /// When set, the field is only shown (and collected/validated) while the
    /// condition holds against another field's current value. `None` → always
    /// visible.
    pub visible_when: Option<FieldCondition>,
}

impl FormFieldSpec {
    fn new(
        key: impl Into<String>,
        label: impl Into<String>,
        kind: FormFieldKind,
        required: bool,
    ) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind,
            required,
            default: None,
            masked: false,
            visible_when: None,
        }
    }

    /// A required free-text field.
    pub fn text(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new(key, label, FormFieldKind::Text, true)
    }

    /// A required single-choice select field.
    pub fn select(key: impl Into<String>, label: impl Into<String>, options: Vec<String>) -> Self {
        Self::new(key, label, FormFieldKind::Select { options }, true)
    }

    /// A boolean toggle field (never blocks submit).
    pub fn toggle(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new(key, label, FormFieldKind::Toggle, false)
    }

    /// A required natural-language date/time field.
    pub fn datetime(key: impl Into<String>, label: impl Into<String>, with_time: bool) -> Self {
        Self::new(key, label, FormFieldKind::DateTime { with_time }, true)
    }

    /// A required whole-number field, unbounded until
    /// [`with_range`](Self::with_range) says otherwise.
    pub fn number(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new(
            key,
            label,
            FormFieldKind::Number {
                min: None,
                max: None,
                step: None,
            },
            true,
        )
    }

    /// A required fractional-number field.
    pub fn decimal(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new(
            key,
            label,
            FormFieldKind::Decimal {
                min: None,
                max: None,
                precision: None,
            },
            true,
        )
    }

    /// A required duration field (submits whole minutes).
    pub fn duration(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new(key, label, FormFieldKind::Duration, true)
    }

    /// A required multiple-choice field.
    pub fn multiselect(
        key: impl Into<String>,
        label: impl Into<String>,
        options: Vec<String>,
    ) -> Self {
        Self::new(key, label, FormFieldKind::MultiSelect { options }, true)
    }

    /// A required multi-line text field, four rows tall until
    /// [`with_lines`](Self::with_lines) says otherwise.
    pub fn multiline(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new(key, label, FormFieldKind::Multiline { lines: None }, true)
    }

    /// Sets how many rows a [`FormFieldKind::Multiline`] box is given. No-op
    /// for any other kind.
    pub fn with_lines(mut self, lines: u16) -> Self {
        if let FormFieldKind::Multiline { lines: l } = &mut self.kind {
            *l = Some(lines);
        }
        self
    }

    /// Bounds a numeric field. Both ends are optional and the values are read
    /// in the field's own arithmetic: rounded to whole numbers for a
    /// [`FormFieldKind::Number`], taken as they are for a
    /// [`FormFieldKind::Decimal`]. No-op for any other kind.
    pub fn with_range(mut self, min: Option<f64>, max: Option<f64>) -> Self {
        match &mut self.kind {
            FormFieldKind::Number {
                min: lo, max: hi, ..
            } => {
                *lo = min.map(|v| v.round() as i64);
                *hi = max.map(|v| v.round() as i64);
            }
            FormFieldKind::Decimal {
                min: lo, max: hi, ..
            } => {
                *lo = min;
                *hi = max;
            }
            _ => {}
        }
        self
    }

    /// Sets what the up/down keys add and subtract on a
    /// [`FormFieldKind::Number`]. No-op for any other kind.
    pub fn with_step(mut self, step: i64) -> Self {
        if let FormFieldKind::Number { step: s, .. } = &mut self.kind {
            *s = Some(step);
        }
        self
    }

    /// Caps the digits after the separator on a [`FormFieldKind::Decimal`],
    /// enforced while typing. No-op for any other kind.
    pub fn with_precision(mut self, precision: u8) -> Self {
        if let FormFieldKind::Decimal { precision: p, .. } = &mut self.kind {
            *p = Some(precision);
        }
        self
    }

    /// Marks the field as optional (submit no longer blocks on an empty value).
    pub fn optional(mut self) -> Self {
        self.required = false;
        self
    }

    /// Explicitly sets the required flag.
    pub fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    /// Masks the field's value on screen (passwords, tokens). No-op for
    /// anything but a text field.
    pub fn masked(mut self) -> Self {
        self.masked = true;
        self
    }

    /// Sets an initial value for the field.
    pub fn with_default(mut self, default: impl Into<String>) -> Self {
        self.default = Some(default.into());
        self
    }

    /// Show this field only while `field`'s value equals `value`.
    pub fn visible_when(mut self, field: impl Into<String>, value: impl Into<String>) -> Self {
        self.visible_when = Some(FieldCondition {
            field: field.into(),
            equals_any: vec![value.into()],
            negate: false,
        });
        self
    }

    /// Show this field only while `field`'s value is one of `values`.
    pub fn visible_when_any(
        mut self,
        field: impl Into<String>,
        values: impl IntoIterator<Item = String>,
    ) -> Self {
        self.visible_when = Some(FieldCondition {
            field: field.into(),
            equals_any: values.into_iter().collect(),
            negate: false,
        });
        self
    }

    /// Show this field only while `field`'s value is *not* `value`.
    pub fn hidden_when(mut self, field: impl Into<String>, value: impl Into<String>) -> Self {
        self.visible_when = Some(FieldCondition {
            field: field.into(),
            equals_any: vec![value.into()],
            negate: true,
        });
        self
    }
}
