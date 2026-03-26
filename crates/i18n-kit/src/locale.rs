use std::fmt::{self, Display, Formatter};

const LOCALE_MAX_BYTES: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Locale {
    bytes: [u8; LOCALE_MAX_BYTES],
    len: u8,
}

impl Locale {
    pub const EN_US: Self = Self::from_static("en_US");
    pub const ZH_CN: Self = Self::from_static("zh_CN");
    pub const JA_JP: Self = Self::from_static("ja_JP");

    #[must_use]
    pub const fn from_static(value: &'static str) -> Self {
        assert!(
            is_valid_canonical_locale_id(value),
            "locale must use canonical language[_Script][_REGION] form"
        );
        Self::from_bytes(value.as_bytes())
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let normalized = normalize_locale_id(value)?;
        Self::from_str(&normalized)
    }

    /// Parses libc/system locale syntax into the catalog's canonical locale form.
    ///
    /// This accepts `language[_Script][_REGION][.codeset][@modifier]`, strips
    /// the codeset, and only understands a small curated set of modifier to
    /// script aliases. It is intentionally not a general BCP47 or CLDR parser.
    /// Unknown modifiers are ignored; malformed or conflicting modifiers are
    /// rejected.
    #[must_use]
    pub fn parse_system(value: &str) -> Option<Self> {
        let normalized = normalize_system_locale_id(value)?;
        Self::from_str(&normalized)
    }

    #[must_use]
    pub fn parse_canonical(value: &str) -> Option<Self> {
        Self::from_str(value)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..usize::from(self.len)])
            .expect("Locale must contain valid UTF-8")
    }

    const fn from_bytes(value: &[u8]) -> Self {
        assert!(
            value.len() <= LOCALE_MAX_BYTES,
            "locale exceeds inline storage capacity"
        );

        let mut bytes = [0; LOCALE_MAX_BYTES];
        let mut index = 0;
        while index < value.len() {
            bytes[index] = value[index];
            index += 1;
        }

        Self {
            bytes,
            len: value.len() as u8,
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        (value.len() <= LOCALE_MAX_BYTES && is_valid_canonical_locale_id(value))
            .then(|| Self::from_bytes(value.as_bytes()))
    }
}

impl fmt::Debug for Locale {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Locale").field(&self.as_str()).finish()
    }
}

impl Display for Locale {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct NormalizedLocaleParts {
    language: String,
    script: Option<String>,
    region: Option<String>,
}

impl NormalizedLocaleParts {
    fn parse(base: &str) -> Option<Self> {
        let mut parts = base.split(['-', '_']);
        let language = normalize_language_subtag(parts.next()?)?;

        match (parts.next(), parts.next(), parts.next()) {
            (None, None, None) => Some(Self {
                language,
                script: None,
                region: None,
            }),
            (Some(second), None, None) => {
                if let Some(region) = normalize_region_subtag(second) {
                    return Some(Self {
                        language,
                        script: None,
                        region: Some(region),
                    });
                }

                Some(Self {
                    language,
                    script: Some(normalize_script_subtag(second)?),
                    region: None,
                })
            }
            (Some(second), Some(third), None) => Some(Self {
                language,
                script: Some(normalize_script_subtag(second)?),
                region: Some(normalize_region_subtag(third)?),
            }),
            _ => None,
        }
    }

    fn to_canonical_string(&self) -> String {
        canonical_locale_id(
            self.language.as_str(),
            self.script.as_deref(),
            self.region.as_deref(),
        )
    }

    fn likely_script_alias(&self) -> Option<&'static str> {
        if self.language != "zh" || self.script.is_some() {
            return None;
        }

        match self.region.as_deref()? {
            "CN" | "SG" | "MY" => Some("Hans"),
            "TW" | "HK" | "MO" => Some("Hant"),
            _ => None,
        }
    }

    fn push_locale_candidate(
        &self,
        candidates: &mut Vec<Locale>,
        script: Option<&str>,
        region: Option<&str>,
    ) {
        let locale_id = canonical_locale_id(self.language.as_str(), script, region);
        let locale = Locale::parse_canonical(&locale_id)
            .expect("normalized locale parts must produce canonical locale ids");

        if !candidates.contains(&locale) {
            candidates.push(locale);
        }
    }
}

fn canonical_locale_id(language: &str, script: Option<&str>, region: Option<&str>) -> String {
    let capacity = language.len()
        + script.map_or(0, |script| script.len() + 1)
        + region.map_or(0, |region| region.len() + 1);
    let mut value = String::with_capacity(capacity);
    value.push_str(language);
    if let Some(script) = script {
        value.push('_');
        value.push_str(script);
    }
    if let Some(region) = region {
        value.push('_');
        value.push_str(region);
    }
    value
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocaleNormalizationKind {
    Strict,
    System,
}

fn normalize_locale_id(value: &str) -> Option<String> {
    Some(normalize_locale_request(value)?.to_canonical_string())
}

fn normalize_system_locale_id(value: &str) -> Option<String> {
    Some(normalize_system_locale_request(value)?.to_canonical_string())
}

pub(crate) fn normalize_locale_request(value: &str) -> Option<NormalizedLocaleParts> {
    normalize_locale_request_with_kind(value, LocaleNormalizationKind::Strict)
}

pub(crate) fn normalize_system_locale_request(value: &str) -> Option<NormalizedLocaleParts> {
    normalize_locale_request_with_kind(value, LocaleNormalizationKind::System)
}

fn normalize_locale_request_with_kind(
    value: &str,
    kind: LocaleNormalizationKind,
) -> Option<NormalizedLocaleParts> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    if kind == LocaleNormalizationKind::Strict {
        return NormalizedLocaleParts::parse(trimmed);
    }

    let (base_with_codeset, modifier) = split_locale_modifier(trimmed)?;
    if !modifier.is_empty() && !is_valid_locale_modifier(modifier) {
        return None;
    }
    let (base, _) = split_locale_codeset(base_with_codeset)?;
    let mut parts = NormalizedLocaleParts::parse(base)?;
    apply_locale_modifier(&mut parts, modifier)?;
    Some(parts)
}

fn split_locale_modifier(value: &str) -> Option<(&str, &str)> {
    match value.split_once('@') {
        Some((base, modifier)) if !base.is_empty() && !modifier.is_empty() => {
            Some((base, modifier))
        }
        Some(_) => None,
        None => Some((value, "")),
    }
}

fn split_locale_codeset(value: &str) -> Option<(&str, &str)> {
    match value.split_once('.') {
        Some((base, codeset)) if !base.is_empty() && !codeset.is_empty() => Some((base, codeset)),
        Some(_) => None,
        None => Some((value, "")),
    }
}

#[cfg(test)]
pub(crate) fn is_posix_default_locale_request(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return false;
    }

    let Some((base_with_codeset, modifier)) = split_locale_modifier(trimmed) else {
        return false;
    };
    let Some((base, _)) = split_locale_codeset(base_with_codeset) else {
        return false;
    };

    let _ = modifier;
    matches!(base, "C" | "POSIX")
}

fn is_valid_locale_modifier(value: &str) -> bool {
    value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
}

fn apply_locale_modifier(parts: &mut NormalizedLocaleParts, modifier: &str) -> Option<()> {
    if modifier.is_empty() || is_ignored_locale_modifier(modifier) {
        return Some(());
    }

    // libc locale modifiers often carry variant or collation hints that this
    // catalog format does not model. Keep the base locale unless the modifier
    // maps to a script we actually understand.
    let Some(script) = locale_modifier_script(parts.language.as_str(), modifier) else {
        return Some(());
    };
    match parts.script.as_deref() {
        Some(existing) if existing == script => Some(()),
        Some(_) => None,
        None => {
            parts.script = Some(script.to_string());
            Some(())
        }
    }
}

fn is_ignored_locale_modifier(value: &str) -> bool {
    value.eq_ignore_ascii_case("euro")
}

fn locale_modifier_script(language: &str, value: &str) -> Option<&'static str> {
    match language {
        "sr" => {
            if value.eq_ignore_ascii_case("latin") {
                return Some("Latn");
            }
            if value.eq_ignore_ascii_case("cyrillic") {
                return Some("Cyrl");
            }
        }
        "zh" => {
            if value.eq_ignore_ascii_case("traditional") {
                return Some("Hant");
            }
            if value.eq_ignore_ascii_case("simplified") {
                return Some("Hans");
            }
        }
        _ => {}
    }

    None
}

const fn is_valid_canonical_locale_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > LOCALE_MAX_BYTES {
        return false;
    }

    let mut first_sep = bytes.len();
    let mut second_sep = bytes.len();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'_' {
            if first_sep == bytes.len() {
                first_sep = index;
            } else if second_sep == bytes.len() {
                second_sep = index;
            } else {
                return false;
            }
        }
        index += 1;
    }

    let first_len = if first_sep == bytes.len() {
        bytes.len()
    } else {
        first_sep
    };
    if !is_lower_alpha(bytes, 0, first_len, 2, 3) {
        return false;
    }
    if first_sep == bytes.len() {
        return true;
    }

    let second_start = first_sep + 1;
    if second_start >= bytes.len() {
        return false;
    }
    let second_len = if second_sep == bytes.len() {
        bytes.len() - second_start
    } else {
        second_sep - second_start
    };
    if second_sep == bytes.len() {
        return is_titlecase_script(bytes, second_start, second_len)
            || is_region(bytes, second_start, second_len);
    }

    let third_start = second_sep + 1;
    if third_start >= bytes.len() {
        return false;
    }
    let third_len = bytes.len() - third_start;
    is_titlecase_script(bytes, second_start, second_len) && is_region(bytes, third_start, third_len)
}

const fn is_lower_alpha(
    bytes: &[u8],
    start: usize,
    len: usize,
    min_len: usize,
    max_len: usize,
) -> bool {
    if len < min_len || len > max_len {
        return false;
    }

    let mut index = 0;
    while index < len {
        let byte = bytes[start + index];
        if !byte.is_ascii_lowercase() {
            return false;
        }
        index += 1;
    }
    true
}

const fn is_titlecase_script(bytes: &[u8], start: usize, len: usize) -> bool {
    if len != 4 || !bytes[start].is_ascii_uppercase() {
        return false;
    }

    let mut index = 1;
    while index < len {
        if !bytes[start + index].is_ascii_lowercase() {
            return false;
        }
        index += 1;
    }
    true
}

const fn is_region(bytes: &[u8], start: usize, len: usize) -> bool {
    if len == 2 {
        let mut index = 0;
        while index < len {
            if !bytes[start + index].is_ascii_uppercase() {
                return false;
            }
            index += 1;
        }
        return true;
    }

    if len == 3 {
        let mut index = 0;
        while index < len {
            if !bytes[start + index].is_ascii_digit() {
                return false;
            }
            index += 1;
        }
        return true;
    }

    false
}

fn normalize_language_subtag(part: &str) -> Option<String> {
    let normalized = part.to_ascii_lowercase();
    ((2..=3).contains(&normalized.len()) && normalized.chars().all(|ch| ch.is_ascii_alphabetic()))
        .then_some(normalized)
}

fn normalize_script_subtag(part: &str) -> Option<String> {
    if part.len() != 4 || !part.chars().all(|ch| ch.is_ascii_alphabetic()) {
        return None;
    }

    let mut normalized = String::with_capacity(4);
    for (index, ch) in part.chars().enumerate() {
        if index == 0 {
            normalized.push(ch.to_ascii_uppercase());
        } else {
            normalized.push(ch.to_ascii_lowercase());
        }
    }
    Some(normalized)
}

fn normalize_region_subtag(part: &str) -> Option<String> {
    ((part.len() == 2 && part.chars().all(|ch| ch.is_ascii_alphabetic()))
        || (part.len() == 3 && part.chars().all(|ch| ch.is_ascii_digit())))
    .then(|| part.to_ascii_uppercase())
}

pub(crate) fn locale_resolution_candidates(parts: &NormalizedLocaleParts) -> Vec<Locale> {
    let mut candidates = Vec::with_capacity(5);
    let script = parts.script.as_deref();
    let region = parts.region.as_deref();

    parts.push_locale_candidate(&mut candidates, script, region);

    if let Some(alias_script) = parts.likely_script_alias() {
        parts.push_locale_candidate(&mut candidates, Some(alias_script), region);
        if region.is_some() {
            parts.push_locale_candidate(&mut candidates, Some(alias_script), None);
        }
    }

    if region.is_some() {
        parts.push_locale_candidate(&mut candidates, script, None);
    }

    if script.is_some() && region.is_some() {
        parts.push_locale_candidate(&mut candidates, None, region);
    }

    if script.is_some() || region.is_some() {
        parts.push_locale_candidate(&mut candidates, None, None);
    }

    candidates
}
