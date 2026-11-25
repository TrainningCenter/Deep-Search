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
            '"' => quoted = !quoted,
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
    let lower = token.to_ascii_lowercase();
    if lower.starts_with("name:") {
        let value = &token[5..];
        if value.is_empty() { return Err("name: requires a value".into()); }
        return Ok(SearchExpression::NameContains(value.into()));
    }
    if lower.starts_with("path:") {
        let value = &token[5..];
        if value.is_empty() { return Err("path: requires a value".into()); }
        return Ok(SearchExpression::PathContains(value.into()));
    }
    if lower.starts_with("content:") {
        let value = &token[8..];
        if value.is_empty() { return Err("content: requires a value".into()); }
        return Ok(SearchExpression::ContentContains(value.into()));
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
    if token.contains(':') || lower.starts_with("size>") || lower.starts_with("size<") {
        return Err(format!("Unknown condition '{token}'. Use name:, path:, content:, ext:, type:, size>= or size<="));
    }
    Ok(SearchExpression::Or(vec![
        SearchExpression::NameContains(token.to_string()),
        SearchExpression::PathContains(token.to_string()),
        SearchExpression::ContentContains(token.to_string()),
    ]))
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
    fn parses_boolean_query_with_parentheses_and_size_units() {
        let parsed = parse_query(r#"name:"holiday video" AND (ext:mkv OR ext:mp4) AND size>=1GB"#).unwrap();
        assert_eq!(parsed, SearchExpression::And(vec![
            SearchExpression::NameContains("holiday video".into()),
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
                SearchExpression::PathContains("project archive".into()),
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
        assert_eq!(
            parse_query("docker").unwrap(),
            SearchExpression::Or(vec![
                SearchExpression::NameContains("docker".into()),
                SearchExpression::PathContains("docker".into()),
                SearchExpression::ContentContains("docker".into()),
            ])
        );
    }

    #[test]
    fn parses_bare_phrases_with_field_filters() {
        assert_eq!(
            parse_query(r#""holiday video" AND ext:mkv"#).unwrap(),
            SearchExpression::And(vec![
                SearchExpression::Or(vec![
                    SearchExpression::NameContains("holiday video".into()),
                    SearchExpression::PathContains("holiday video".into()),
                    SearchExpression::ContentContains("holiday video".into()),
                ]),
                SearchExpression::ExtensionEquals("mkv".into()),
            ])
        );
    }

    #[test]
    fn parses_content_search_condition() {
        assert_eq!(
            parse_query(r#"content:"project roadmap" AND ext:txt"#).unwrap(),
            SearchExpression::And(vec![
                SearchExpression::ContentContains("project roadmap".into()),
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
