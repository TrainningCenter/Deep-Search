use crate::database::SearchExpression;

pub fn parse_query(input: &str) -> Result<SearchExpression, String> {
    let tokens = tokenize(input)?;
    if tokens.is_empty() { return Err("Search query cannot be empty".into()); }
    let mut parser = Parser { tokens, position: 0 };
    let expression = parser.parse_or()?;
    if parser.position != parser.tokens.len() {
        return Err(format!("Unexpected token: {}", parser.tokens[parser.position]));
    }
    Ok(expression)
}

fn tokenize(input: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for ch in input.chars() {
        match ch {
            '"' => {
                if !quoted { current.push('\0'); }
                quoted = !quoted;
            },
            '(' | ')' if !quoted => {
                if !current.is_empty() { tokens.push(std::mem::take(&mut current)); }
                tokens.push(ch.to_string());
            }
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() { tokens.push(std::mem::take(&mut current)); }
            }
            c => current.push(c),
        }
    }
    if quoted { return Err("Unclosed quote in search query".into()); }
    if !current.is_empty() { tokens.push(current); }
    Ok(tokens)
}

struct Parser { tokens: Vec<String>, position: usize }

impl Parser {
    fn peek(&self) -> Option<&str> { self.tokens.get(self.position).map(String::as_str) }

    fn consume(&mut self) -> Option<String> {
        let token = self.tokens.get(self.position).cloned();
        self.position += usize::from(token.is_some());
        token
    }

    fn eat_keyword(&mut self, keyword: &str) -> bool {
        if self.peek().is_some_and(|token| token.eq_ignore_ascii_case(keyword)) {
            self.position += 1;
            true
        } else { false }
    }

    fn parse_or(&mut self) -> Result<SearchExpression, String> {
        let mut items = vec![self.parse_and()?];
        while self.eat_keyword("OR") { items.push(self.parse_and()?); }
        Ok(if items.len() == 1 { items.remove(0) } else { SearchExpression::Or(items) })
    }

    fn parse_and(&mut self) -> Result<SearchExpression, String> {
        let mut items = vec![self.parse_unary()?];
        while self.eat_keyword("AND") { items.push(self.parse_unary()?); }
        Ok(if items.len() == 1 { items.remove(0) } else { SearchExpression::And(items) })
    }

    fn parse_unary(&mut self) -> Result<SearchExpression, String> {
        if self.eat_keyword("NOT") {
            return Ok(SearchExpression::Not(Box::new(self.parse_unary()?)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<SearchExpression, String> {
        if self.peek() == Some("(") {
            self.consume();
            let expression = self.parse_or()?;
            if self.consume().as_deref() != Some(")") { return Err("Missing closing parenthesis".into()); }
            return Ok(expression);
        }
        let token = self.consume().ok_or_else(|| "Expected a search condition".to_string())?;
        if token == ")" { return Err("Unexpected closing parenthesis".into()); }
        parse_condition(&token)
    }
}

fn parse_condition(token: &str) -> Result<SearchExpression, String> {
    let exact = token.contains('\0');
    let cleaned = token.replace('\0', "");
    let token = cleaned.as_str();
    let lower = token.to_ascii_lowercase();
    if lower.starts_with("name:") {
        let value = &token[5..];
        if value.is_empty() { return Err("name: requires a value".into()); }
        return Ok(if exact { SearchExpression::NameExact(value.into()) } else { SearchExpression::NameContains(value.into()) });
    }
    if lower.starts_with("path:") {
        let value = &token[5..];
        if value.is_empty() { return Err("path: requires a value".into()); }
        return Ok(if exact { SearchExpression::PathExact(value.into()) } else { SearchExpression::PathContains(value.into()) });
    }
    if lower.starts_with("source:") {
        let value = &token[7..];
        if value.is_empty() { return Err("source: requires a value".into()); }
        return Ok(SearchExpression::SourceContains(value.into()));
    }
    if lower.starts_with("content:") {
        let value = &token[8..];
        if value.is_empty() { return Err("content: requires a value".into()); }
        return Ok(if exact { SearchExpression::ContentExact(value.into()) } else { SearchExpression::ContentContains(value.into()) });
    }
    if lower.starts_with("ext:") {
        let value = &token[4..];
        if value.is_empty() { return Err("ext: requires a value".into()); }
        return Ok(SearchExpression::ExtensionEquals(value.into()));
    }
    if lower.starts_with("type:") {
        let value = &token[5..];
        if value.is_empty() { return Err("type: requires a value".into()); }
        let normalized = value.to_ascii_lowercase();
        if !["video", "audio", "image", "document", "archive"].contains(&normalized.as_str()) {
            return Err(format!("Unknown file type '{value}'. Use video, audio, image, document, or archive"));
        }
        return Ok(SearchExpression::FileType(normalized));
    }
    if lower.starts_with("size>=") { return Ok(SearchExpression::SizeAtLeast(parse_size(&token[6..])?)); }
    if lower.starts_with("size<=") { return Ok(SearchExpression::SizeAtMost(parse_size(&token[6..])?)); }
    if lower.starts_with("duration>=") { return Ok(SearchExpression::DurationAtLeast(parse_duration(&token[10..])?)); }
    if lower.starts_with("duration<=") { return Ok(SearchExpression::DurationAtMost(parse_duration(&token[10..])?)); }
    if lower.starts_with("resolution>=") {
        let (width, height) = parse_resolution_threshold(&token[12..])?;
        return Ok(SearchExpression::ResolutionAtLeast { min_width: width, min_height: height });
    }
    if lower.starts_with("resolution<=") {
        let (width, height) = parse_resolution_threshold(&token[12..])?;
        return Ok(SearchExpression::ResolutionAtMost { max_width: width, max_height: height });
    }
    if lower.starts_with("resolution:") {
        let value = &token[11..];
        let normalized = value.to_ascii_lowercase();
        if let Some((width, height)) = normalized.split_once('x') {
            let width = parse_resolution_dimension(width)?;
            let height = parse_resolution_dimension(height)?;
            return Ok(SearchExpression::ResolutionEquals { width, height });
        }
        let (width, height) = parse_resolution_threshold(value)?;
        return Ok(SearchExpression::ResolutionAtLeast { min_width: width, min_height: height });
    }
    if lower.starts_with("fps:>=") {
        return Ok(SearchExpression::FpsAtLeast(parse_fps(&token[6..])?));
    }
    if lower.starts_with("fps:<=") {
        return Ok(SearchExpression::FpsAtMost(parse_fps(&token[6..])?));
    }
    if lower.starts_with("fps>=") {
        return Ok(SearchExpression::FpsAtLeast(parse_fps(&token[5..])?));
    }
    if lower.starts_with("fps<=") {
        return Ok(SearchExpression::FpsAtMost(parse_fps(&token[5..])?));
    }
    if lower.starts_with("fps:") {
        let fps = parse_fps(&token[4..])?;
        return Ok(SearchExpression::And(vec![
            SearchExpression::FpsAtLeast(fps),
            SearchExpression::FpsAtMost(fps),
        ]));
    }
    if token.contains(':') || lower.starts_with("size>") || lower.starts_with("size<") || lower.starts_with("duration>") || lower.starts_with("duration<") || lower.starts_with("resolution>") || lower.starts_with("resolution<") || lower.starts_with("fps>") || lower.starts_with("fps<") {
        return Err(format!("Unknown condition '{token}'. Use name:, path:, source:, content:, ext:, type:, size>=, size<=, duration>=, duration<=, resolution:, resolution>=, resolution<=, fps:>= or fps:<="));
    }
    if exact {
        if token.trim().is_empty() { return Err("Quoted search terms cannot be empty".into()); }
        Ok(SearchExpression::ExactTerm(token.to_string()))
    } else {
        Ok(SearchExpression::ExpandedTerm(token.to_string()))
    }
}

fn parse_resolution_dimension(value: &str) -> Result<u32, String> {
    let dimension = value.trim().parse::<u32>()
        .map_err(|_| format!("Invalid resolution dimension: {value}"))?;
    if dimension == 0 { return Err("Resolution dimensions must be greater than zero".into()); }
    Ok(dimension)
}

fn parse_resolution_threshold(value: &str) -> Result<(Option<u32>, Option<u32>), String> {
    let normalized = value.trim().to_ascii_lowercase();
    if let Some((width, height)) = normalized.split_once('x') {
        return Ok((Some(parse_resolution_dimension(width)?), Some(parse_resolution_dimension(height)?)));
    }
    let threshold = match normalized.as_str() {
        "8k" => (Some(7680), None),
        "4k" | "uhd" => (Some(3840), None),
        "2160p" => (None, Some(2160)),
        "1440p" | "2k" => (None, Some(1440)),
        "1080p" | "fullhd" | "fhd" => (None, Some(1080)),
        "720p" | "hd" => (None, Some(720)),
        "480p" => (None, Some(480)),
        _ => {
            let width = parse_resolution_dimension(&normalized)?;
            (Some(width), None)
        }
    };
    Ok(threshold)
}

fn parse_fps(value: &str) -> Result<u32, String> {
    let normalized = value.trim().to_ascii_lowercase();
    let normalized = normalized.strip_suffix("fps").unwrap_or(&normalized).trim();
    let fps = normalized.parse::<f64>()
        .map_err(|_| format!("Invalid frame rate: {value}"))?;
    if !fps.is_finite() || fps <= 0.0 || fps > 1000.0 {
        return Err(format!("Invalid frame rate: {value}"));
    }
    Ok((fps * 1000.0).round() as u32)
}

fn parse_duration(value: &str) -> Result<u64, String> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty() {
        return Err("Duration requires a value".into());
    }
    if value.contains(':') {
        let parts = value.split(':').collect::<Vec<_>>();
        if !(2..=3).contains(&parts.len()) {
            return Err(format!("Invalid duration: {value}"));
        }
        let numbers = parts.iter().map(|part| part.parse::<f64>()
            .map_err(|_| format!("Invalid duration: {value}")))
            .collect::<Result<Vec<_>, _>>()?;
        if numbers.iter().any(|number| !number.is_finite() || *number < 0.0) {
            return Err(format!("Invalid duration: {value}"));
        }
        let seconds = if numbers.len() == 2 {
            numbers[0] * 60.0 + numbers[1]
        } else {
            numbers[0] * 3600.0 + numbers[1] * 60.0 + numbers[2]
        };
        if numbers.len() == 2 && numbers[1] >= 60.0
            || numbers.len() == 3 && (numbers[1] >= 60.0 || numbers[2] >= 60.0)
        {
            return Err(format!("Invalid duration: {value}"));
        }
        let milliseconds = seconds * 1000.0;
        if milliseconds > u64::MAX as f64 { return Err("Duration is too large".into()); }
        return Ok(milliseconds.round() as u64);
    }
    let split = value.find(|ch: char| !ch.is_ascii_digit() && ch != '.').unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    let amount: f64 = number.parse().map_err(|_| format!("Invalid duration: {value}"))?;
    if !amount.is_finite() || amount < 0.0 { return Err(format!("Invalid duration: {value}")); }
    let multiplier = match unit.trim() {
        "" | "s" | "sec" | "secs" | "second" | "seconds" => 1_000.0,
        "ms" | "millisecond" | "milliseconds" => 1.0,
        "m" | "min" | "mins" | "minute" | "minutes" => 60_000.0,
        "h" | "hr" | "hrs" | "hour" | "hours" => 3_600_000.0,
        _ => return Err(format!("Unknown duration unit: {unit}")),
    };
    let milliseconds = amount * multiplier;
    if milliseconds > u64::MAX as f64 { return Err("Duration is too large".into()); }
    Ok(milliseconds.round() as u64)
}

fn parse_size(value: &str) -> Result<u64, String> {
    let value = value.trim();
    let split = value.find(|ch: char| !ch.is_ascii_digit() && ch != '.').unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    let amount: f64 = number.parse().map_err(|_| format!("Invalid file size: {value}"))?;
    if !amount.is_finite() || amount < 0.0 { return Err(format!("Invalid file size: {value}")); }
    let multiplier = match unit.to_ascii_lowercase().as_str() {
        "" | "b" => 1_f64,
        "k" | "kb" | "kib" => 1024_f64,
        "m" | "mb" | "mib" => 1024_f64.powi(2),
        "g" | "gb" | "gib" => 1024_f64.powi(3),
        "t" | "tb" | "tib" => 1024_f64.powi(4),
        _ => return Err(format!("Unknown size unit: {unit}")),
    };
    let bytes = amount * multiplier;
    if bytes > u64::MAX as f64 { return Err("File size is too large".into()); }
    Ok(bytes.round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinguishes_expanded_unquoted_terms_from_exact_quoted_terms() {
        assert_eq!(parse_query("invoice").unwrap(), SearchExpression::ExpandedTerm("invoice".into()));
        assert_eq!(parse_query("\"invoice\"").unwrap(), SearchExpression::ExactTerm("invoice".into()));
        assert_eq!(parse_query("name:\"invoice\"").unwrap(), SearchExpression::NameExact("invoice".into()));
        assert_eq!(parse_query("content:\"holiday video\"").unwrap(), SearchExpression::ContentExact("holiday video".into()));
    }

    #[test]
    fn parses_resolution_and_frame_rate_filters() {
        assert_eq!(
            parse_query("resolution:4K").unwrap(),
            SearchExpression::ResolutionAtLeast { min_width: Some(3840), min_height: None }
        );
        assert_eq!(
            parse_query("resolution:1920x1080").unwrap(),
            SearchExpression::ResolutionEquals { width: 1920, height: 1080 }
        );
        assert_eq!(
            parse_query("resolution:1920X1080").unwrap(),
            SearchExpression::ResolutionEquals { width: 1920, height: 1080 }
        );
        assert_eq!(
            parse_query("resolution<=1920x1080").unwrap(),
            SearchExpression::ResolutionAtMost { max_width: Some(1920), max_height: Some(1080) }
        );
        assert_eq!(
            parse_query("resolution<=1080p").unwrap(),
            SearchExpression::ResolutionAtMost { max_width: None, max_height: Some(1080) }
        );
        assert_eq!(parse_query("fps:>=60").unwrap(), SearchExpression::FpsAtLeast(60_000));
        assert_eq!(parse_query("fps:<=29.97").unwrap(), SearchExpression::FpsAtMost(29_970));
        assert_eq!(
            parse_query("fps:60").unwrap(),
            SearchExpression::And(vec![SearchExpression::FpsAtLeast(60_000), SearchExpression::FpsAtMost(60_000)])
        );
    }

    #[test]
    fn rejects_invalid_resolution_and_frame_rate_filters() {
        assert!(parse_query("resolution:0x1080").is_err());
        assert!(parse_query("fps:0").is_err());
        assert!(parse_query("fps:1001").is_err());
    }

    #[test]
    fn parses_source_filter() {
        assert_eq!(
            parse_query("source:nas").unwrap(),
            SearchExpression::SourceContains("nas".into())
        );
    }

    #[test]
    fn parses_duration_filters_in_seconds_minutes_and_clock_format() {
        assert_eq!(parse_query("duration>=90s").unwrap(), SearchExpression::DurationAtLeast(90_000));
        assert_eq!(parse_query("duration<=2h").unwrap(), SearchExpression::DurationAtMost(7_200_000));
        assert_eq!(parse_query("duration>=1:30").unwrap(), SearchExpression::DurationAtLeast(90_000));
        assert_eq!(parse_query("duration<=01:02:03").unwrap(), SearchExpression::DurationAtMost(3_723_000));
    }

    #[test]
    fn rejects_invalid_duration_filters() {
        assert!(parse_query("duration>=2fortnights").is_err());
        assert!(parse_query("duration>=1:90").is_err());
    }

    #[test]
    fn parses_boolean_query_with_parentheses_and_size_units() {
        let parsed = parse_query(r#"name:"holiday video" AND (ext:mkv OR ext:mp4) AND size>=1GB"#).unwrap();
        assert_eq!(parsed, SearchExpression::And(vec![
            SearchExpression::NameExact("holiday video".into()),
            SearchExpression::Or(vec![
                SearchExpression::ExtensionEquals("mkv".into()),
                SearchExpression::ExtensionEquals("mp4".into()),
            ]),
            SearchExpression::SizeAtLeast(1024_u64.pow(3)),
        ]));
    }

    #[test]
    fn parses_not_and_size_upper_bound() {
        let parsed = parse_query("NOT ext:tmp AND size<=500MB").unwrap();
        assert_eq!(parsed, SearchExpression::And(vec![
            SearchExpression::Not(Box::new(SearchExpression::ExtensionEquals("tmp".into()))),
            SearchExpression::SizeAtMost(500 * 1024 * 1024),
        ]));
    }

    #[test]
    fn parses_path_search_condition() {
        assert_eq!(
            parse_query(r#"path:"project archive" AND ext:pdf"#).unwrap(),
            SearchExpression::And(vec![
                SearchExpression::PathExact("project archive".into()),
                SearchExpression::ExtensionEquals("pdf".into()),
            ])
        );
    }

    #[test]
    fn parses_file_type_search_condition() {
        assert_eq!(
            parse_query("type:video AND ext:mkv").unwrap(),
            SearchExpression::And(vec![
                SearchExpression::FileType("video".into()),
                SearchExpression::ExtensionEquals("mkv".into()),
            ])
        );
        assert!(parse_query("type:camera").is_err());
        assert!(parse_query("type:").is_err());
    }

    #[test]
    fn parses_bare_terms_across_name_path_and_content() {
        assert_eq!(parse_query("docker").unwrap(), SearchExpression::ExpandedTerm("docker".into()));
    }

    #[test]
    fn parses_bare_phrases_with_field_filters() {
        assert_eq!(
            parse_query(r#""holiday video" AND ext:mkv"#).unwrap(),
            SearchExpression::And(vec![
                SearchExpression::ExactTerm("holiday video".into()),
                SearchExpression::ExtensionEquals("mkv".into()),
            ])
        );
    }

    #[test]
    fn parses_content_search_condition() {
        assert_eq!(
            parse_query(r#"content:"project roadmap" AND ext:txt"#).unwrap(),
            SearchExpression::And(vec![
                SearchExpression::ContentExact("project roadmap".into()),
                SearchExpression::ExtensionEquals("txt".into()),
            ])
        );
    }

    #[test]
    fn rejects_unclosed_quotes_and_unknown_conditions() {
        assert!(parse_query(r#"name:"unfinished"#).is_err());
        assert!(parse_query("kind:video").is_err());
    }
}
