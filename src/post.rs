use serde::Serialize;
use serde_json::Value;

/// The projection of an upstream blog post that this proxy exposes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Post {
    pub id: u64,
    pub name: String,
    pub date: String,
    pub url: String,
    pub thumbnail: Option<String>,
}

/// Parse the WordPress REST API posts response into `Post`s.
///
/// Posts are extracted leniently: an entry that is missing an id or a title is
/// skipped rather than failing the whole batch, matching the upstream API's
/// occasional rough edges.
pub fn parse_posts(text: &str) -> Result<Vec<Post>, serde_json::Error> {
    let raw: Vec<Value> = serde_json::from_str(text)?;
    Ok(raw.into_iter().filter_map(normalize).collect())
}

fn normalize(value: Value) -> Option<Post> {
    let id = value.get("id")?.as_u64()?;
    let name = decode_entities(
        value
            .get("title")
            .and_then(|title| title.get("rendered"))
            .and_then(Value::as_str)?,
    );
    let date = value
        .get("date")
        .and_then(Value::as_str)
        .or_else(|| value.get("date_gmt").and_then(Value::as_str))
        .unwrap_or_default()
        .to_string();
    let url = value
        .get("link")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    Some(Post {
        id,
        name,
        date,
        url,
        thumbnail: featured_thumbnail(&value),
    })
}

/// The URL of the post's featured image, preferring the `thumbnail` size and
/// falling back to the other registered sizes, then the full-size source.
fn featured_thumbnail(value: &Value) -> Option<String> {
    const PREFERRED_SIZES: [&str; 4] = ["thumbnail", "medium", "large", "full"];

    let media = value
        .get("_embedded")
        .and_then(|embedded| embedded.get("wp:featuredmedia"))
        .and_then(Value::as_array)
        .and_then(|media| media.first())?;

    let sizes = media
        .get("media_details")
        .and_then(|details| details.get("sizes"));
    for size in PREFERRED_SIZES {
        if let Some(url) = sizes
            .and_then(|sizes| sizes.get(size))
            .and_then(|size| size.get("source_url"))
            .and_then(Value::as_str)
        {
            return Some(url.to_string());
        }
    }

    media
        .get("source_url")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The longest name we treat as an entity (`#x1F600` and the longest HTML5
/// names fit well within this).
const MAX_ENTITY_NAME: usize = 32;

/// Decode the HTML entities WordPress emits in rendered titles.
///
/// Numeric references (`&#8217;`, `&#x1F600;`) and common named entities
/// (`&amp;`, `&hellip;`, …) are decoded. Anything unrecognised — or an `&` with
/// no nearby `;` — is left as-is, so plain text is never mangled.
fn decode_entities(input: &str) -> String {
    if !input.contains('&') {
        return input.to_string();
    }

    let mut out = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];

        // Only treat this as an entity if `&` is followed by a plausible name
        // and a nearby `;`; otherwise it is a literal ampersand.
        let entity = after
            .find(';')
            .filter(|&end| end > 0 && end <= MAX_ENTITY_NAME && is_entity_name(&after[..end]))
            .and_then(|end| decode_entity(&after[..end]).map(|decoded| (decoded, end)));

        match entity {
            Some((decoded, end)) => {
                out.push(decoded);
                rest = &after[end + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }

    out.push_str(rest);
    out
}

/// Whether `name` (the text between `&` and `;`) could be an entity name.
fn is_entity_name(name: &str) -> bool {
    name.chars().all(|c| c.is_ascii_alphanumeric() || c == '#')
}

/// Decode a single entity name (the text between `&` and `;`).
fn decode_entity(name: &str) -> Option<char> {
    if let Some(number) = name.strip_prefix('#') {
        let code = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => number.parse::<u32>().ok()?,
        };
        return char::from_u32(code);
    }

    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "hellip" => '…',
        "mdash" => '—',
        "ndash" => '–',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "sbquo" => '‚',
        "bdquo" => '„',
        "laquo" => '«',
        "raquo" => '»',
        "bull" => '•',
        "middot" => '·',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "deg" => '°',
        "times" => '×',
        "divide" => '÷',
        "plusmn" => '±',
        "micro" => 'µ',
        "para" => '¶',
        "sect" => '§',
        "cent" => '¢',
        "pound" => '£',
        "euro" => '€',
        "yen" => '¥',
        "frac14" => '¼',
        "frac12" => '½',
        "frac34" => '¾',
        "sup1" => '¹',
        "sup2" => '²',
        "sup3" => '³',
        "szlig" => 'ß',
        "aelig" => 'æ',
        "oslash" => 'ø',
        "ccedil" => 'ç',
        "ntilde" => 'ñ',
        "eacute" => 'é',
        "egrave" => 'è',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_only_the_exposed_fields() {
        let json = r#"[{
            "id": 7,
            "date": "2026-01-02T03:04:05",
            "link": "https://example.test/2026/01/hello/",
            "title": { "rendered": "Hello &amp; welcome" },
            "content": { "rendered": "<p>hi</p>" },
            "featured_media": 99,
            "_embedded": {
                "wp:featuredmedia": [
                    {
                        "source_url": "full.jpg",
                        "media_details": {
                            "sizes": {
                                "thumbnail": { "source_url": "thumb.jpg" },
                                "large": { "source_url": "large.jpg" }
                            }
                        }
                    }
                ]
            }
        }]"#;

        let posts = parse_posts(json).unwrap();
        assert_eq!(
            posts,
            vec![Post {
                id: 7,
                name: "Hello & welcome".to_string(),
                date: "2026-01-02T03:04:05".to_string(),
                url: "https://example.test/2026/01/hello/".to_string(),
                thumbnail: Some("thumb.jpg".to_string()),
            }]
        );
    }

    #[test]
    fn falls_back_through_image_sizes_then_source() {
        let cases = [
            (r#"{"medium":{"source_url":"m.jpg"}}"#, "m.jpg"),
            (r#"{"large":{"source_url":"l.jpg"}}"#, "l.jpg"),
            (r#"{"full":{"source_url":"f.jpg"}}"#, "f.jpg"),
            (r#"{}"#, "src.jpg"),
        ];

        for (sizes, expected) in cases {
            let json = format!(
                r#"[{{"id":1,"link":"u","title":{{"rendered":"t"}},"_embedded":{{"wp:featuredmedia":[{{"source_url":"src.jpg","media_details":{{"sizes":{sizes}}}}}]}}}}]"#
            );
            let posts = parse_posts(&json).unwrap();
            assert_eq!(posts[0].thumbnail.as_deref(), Some(expected));
        }
    }

    #[test]
    fn tolerates_missing_data() {
        let posts = parse_posts(
            r#"[{"id":1,"link":"u","title":{"rendered":"A"}},{"id":2,"link":"u","title":{"rendered":"B"},"_embedded":{"wp:featuredmedia":[]}},{"link":"u","title":{"rendered":"no id"}},{"id":3,"link":"u"}]"#,
        )
        .unwrap();

        assert_eq!(posts.len(), 2, "entries without an id or title are skipped");
        assert_eq!(posts[0].thumbnail, None);
        assert_eq!(posts[1].thumbnail, None);
        assert_eq!(posts[0].date, "");
    }

    #[test]
    fn falls_back_to_gmt_date() {
        let posts =
            parse_posts(r#"[{"id":1,"date_gmt":"2026-01-02T03:04:05","title":{"rendered":"t"}}]"#)
                .unwrap();
        assert_eq!(posts[0].date, "2026-01-02T03:04:05");
    }

    #[test]
    fn decodes_numeric_and_named_entities() {
        assert_eq!(decode_entities("It&#8217;s here"), "It’s here");
        assert_eq!(decode_entities("caf&#233;"), "café");
        assert_eq!(decode_entities("&#x1F600;!"), "😀!");
        assert_eq!(decode_entities("Fish &amp; Chips"), "Fish & Chips");
        assert_eq!(decode_entities("A &hellip; B &mdash; C"), "A … B — C");
    }

    #[test]
    fn leaves_plain_text_and_unknown_entities_untouched() {
        assert_eq!(decode_entities("no entities here"), "no entities here");
        assert_eq!(
            decode_entities("R&D &unknown; 5 & 6"),
            "R&D &unknown; 5 & 6"
        );
        assert_eq!(decode_entities("&#xZZ;"), "&#xZZ;");
    }

    #[test]
    fn rejects_a_non_array_payload() {
        assert!(parse_posts(r#"{"oops":true}"#).is_err());
    }
}
