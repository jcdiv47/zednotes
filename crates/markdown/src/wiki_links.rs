use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};
use std::{collections::HashMap, ops::Range};
use util::markdown::generate_heading_slug;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WikiLink {
    pub target: String,
    pub range: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heading {
    pub title: String,
    pub slug: String,
    pub offset: usize,
}

#[derive(Clone, Debug, Default)]
pub struct NoteLinks {
    pub links: Vec<WikiLink>,
    pub headings: Vec<Heading>,
}

const OPTIONS: Options =
    super::parser::PARSE_OPTIONS.union(Options::ENABLE_YAML_STYLE_METADATA_BLOCKS);

pub fn parse(source: &str) -> NoteLinks {
    let mut result = NoteLinks::default();
    let mut heading: Option<(usize, String)> = None;
    let mut slug_counts = HashMap::<String, usize>::new();
    for (event, range) in Parser::new_ext(source, OPTIONS).into_offset_iter() {
        match event {
            Event::Start(Tag::Link {
                link_type: LinkType::WikiLink { .. },
                dest_url,
                ..
            }) => result.links.push(WikiLink {
                target: dest_url.trim().to_owned(),
                range,
            }),
            Event::Start(Tag::Heading { .. }) => heading = Some((range.start, String::new())),
            Event::Text(text) | Event::Code(text) => {
                if let Some((_, title)) = &mut heading {
                    title.push_str(&text);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some((_, title)) = &mut heading {
                    title.push(' ');
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((offset, title)) = heading.take() {
                    let base = generate_heading_slug(&title);
                    let count = slug_counts.entry(base.clone()).or_default();
                    let mut slug = if *count == 0 {
                        base.clone()
                    } else {
                        format!("{base}-{count}")
                    };
                    *count += 1;
                    while result.headings.iter().any(|heading| heading.slug == slug) {
                        slug = format!("{base}-{count}");
                        *count += 1;
                    }
                    result.headings.push(Heading {
                        title,
                        slug,
                        offset,
                    });
                }
            }
            _ => {}
        }
    }
    result
}

pub fn split_target(target: &str) -> (&str, Option<&str>) {
    let target = target.trim();
    match target.split_once('#') {
        Some((path, heading)) => (path.trim(), Some(heading.trim())),
        None => (target, None),
    }
}

impl NoteLinks {
    pub fn link_at(&self, offset: usize) -> Option<&WikiLink> {
        self.links.iter().find(|link| link.range.contains(&offset))
    }

    pub fn heading_offset(&self, fragment: &str) -> Option<usize> {
        let slug = generate_heading_slug(fragment);
        self.headings
            .iter()
            .find(|heading| heading.title == fragment)
            .or_else(|| self.headings.iter().find(|heading| heading.slug == slug))
            .map(|heading| heading.offset)
    }
}

/// Returns the target being completed, excluding brackets, an alias, or a closing delimiter.
pub fn completion_range(source: &str, offset: usize) -> Option<Range<usize>> {
    let prefix = source.get(..offset)?;
    let line_start = prefix.rfind('\n').map_or(0, |index| index + 1);
    let opening = prefix.get(line_start..)?.rfind("[[")? + line_start;
    let target = source.get(opening + 2..offset)?;
    if target.contains(['[', ']', '|']) {
        return None;
    }
    // Parse a completed probe so code, escaped brackets, HTML, and math are excluded
    // by the same parser used to navigate and index links.
    let mut probe = source.get(..opening + 2)?.to_owned();
    probe.push_str("note]]");
    probe.push_str(source.get(offset..)?);
    if !parse(&probe)
        .links
        .iter()
        .any(|link| link.range.start == opening)
    {
        return None;
    }
    Some(opening + 2..offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_aliases_headings_and_unicode_with_source_ranges() {
        let source = "See [[数学#向量|vectors]] and [[folder/Note]].";
        let parsed = parse(source);
        assert_eq!(parsed.links.len(), 2);
        assert_eq!(parsed.links[0].target, "数学#向量");
        assert_eq!(
            &source[parsed.links[0].range.clone()],
            "[[数学#向量|vectors]]"
        );
        assert_eq!(parsed.links[1].target, "folder/Note");
    }

    #[test]
    fn ignores_code_math_metadata_and_escaped_links() {
        let source = "---\ntitle: '[[metadata]]'\n---\n`[[inline]]`\n\n```md\n[[fenced]]\n```\n\n    [[indented]]\n\n$[[math]]$ and \\[[escaped]]\n\n[[real]]";
        assert_eq!(
            parse(source)
                .links
                .iter()
                .map(|link| link.target.as_str())
                .collect::<Vec<_>>(),
            vec!["real"]
        );
    }

    #[test]
    fn headings_support_formatted_titles_and_duplicate_slugs() {
        let source = "# A **heading**\n\n# A heading\n\n# A heading-1\n\n## 数学";
        let parsed = parse(source);
        assert_eq!(parsed.heading_offset("A heading"), Some(0));
        assert_eq!(
            parsed.heading_offset("a-heading-1"),
            source.find("# A heading\n")
        );
        assert_eq!(
            parsed
                .headings
                .iter()
                .map(|heading| heading.slug.as_str())
                .collect::<Vec<_>>(),
            vec!["a-heading", "a-heading-1", "a-heading-1-1", "数学"]
        );
    }

    #[test]
    fn completes_only_wiki_link_targets() {
        for source in ["[[", "[[folder/", "[[Note#", "See [[数学"] {
            assert!(completion_range(source, source.len()).is_some(), "{source}");
        }
        for source in ["```\n[[code", "\\[[escaped", "[[Note|alias", "[[done]]"] {
            assert!(completion_range(source, source.len()).is_none(), "{source}");
        }
        assert!(completion_range("`[[code`", 7).is_none());
        assert!(completion_range("$[[math$", 7).is_none());
    }
}
