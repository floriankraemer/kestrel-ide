//! A tiny XML-to-tree reader shared by [`super::pom`] and
//! [`super::effective_pom`] (A5).
//!
//! Every other XML consumer in this repo (`analysis-core::checkstyle`,
//! `test-core::junit`) hand-rolls a state machine over `quick_xml::events`,
//! which is right for their flat, single-shape documents. A POM is a real
//! tree — `<dependencies><dependency><exclusions><exclusion>` nests as deep
//! as an author wants — so this builds a minimal in-memory [`Node`] once and
//! lets both Maven readers query it by child name, rather than each writing
//! its own nested-state machine over the same document shape.

use std::fmt;

use quick_xml::events::Event;
use quick_xml::Reader;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Node {
    pub name: String,
    pub children: Vec<Node>,
    pub text: String,
}

#[derive(Debug)]
pub struct XmlError(pub String);

impl fmt::Display for XmlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "malformed XML: {}", self.0)
    }
}

impl Node {
    /// The first direct child named `name`, if any.
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    /// Every direct child named `name`, in document order.
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// A direct child's own trimmed text, when both the child and the text
    /// exist and are non-empty.
    pub fn text_of(&self, name: &str) -> Option<String> {
        self.child(name)
            .map(|c| c.text.trim().to_string())
            .filter(|t| !t.is_empty())
    }
}

/// Parse `text` into a tree rooted at the document's single top-level
/// element (`<project>` for a POM, `<plugin>` for a `plugin.xml`).
pub fn parse(text: &str) -> Result<Node, XmlError> {
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);

    let mut stack: Vec<Node> = Vec::new();
    let mut root: Option<Node> = None;
    let mut buf = Vec::new();

    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(tag)) => {
                let name = local_name(tag.name().as_ref());
                stack.push(Node {
                    name,
                    children: Vec::new(),
                    text: String::new(),
                });
            }
            Ok(Event::Empty(tag)) => {
                let name = local_name(tag.name().as_ref());
                let node = Node {
                    name,
                    children: Vec::new(),
                    text: String::new(),
                };
                attach(&mut stack, &mut root, node);
            }
            Ok(Event::Text(text)) => {
                if let Some(top) = stack.last_mut() {
                    let raw = text.decode().map_err(|err| XmlError(err.to_string()))?;
                    let decoded = quick_xml::escape::unescape(&raw)
                        .map(|s| s.into_owned())
                        .unwrap_or_else(|_| raw.into_owned());
                    top.text.push_str(&decoded);
                }
            }
            Ok(Event::CData(text)) => {
                if let Some(top) = stack.last_mut() {
                    let raw = text.into_inner();
                    top.text.push_str(&String::from_utf8_lossy(&raw));
                }
            }
            Ok(Event::End(_)) => {
                let Some(node) = stack.pop() else {
                    return Err(XmlError("unbalanced closing tag".to_string()));
                };
                attach(&mut stack, &mut root, node);
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(err) => return Err(XmlError(err.to_string())),
        }
    }

    root.ok_or_else(|| XmlError("no root element".to_string()))
}

fn attach(stack: &mut [Node], root: &mut Option<Node>, node: Node) {
    match stack.last_mut() {
        Some(parent) => parent.children.push(node),
        None => *root = Some(node),
    }
}

/// Strips a namespace prefix (`xsi:foo` -> `foo`) — a POM's own elements
/// carry none, but being defensive here costs nothing and saves a surprise
/// on a hand-edited file that added one.
fn local_name(qualified: &[u8]) -> String {
    let text = String::from_utf8_lossy(qualified);
    text.rsplit(':').next().unwrap_or(&text).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nested_document_builds_the_matching_tree() {
        let root = parse(
            r#"<project>
                <groupId>com.example</groupId>
                <dependencies>
                    <dependency>
                        <groupId>org.junit.jupiter</groupId>
                        <artifactId>junit-jupiter</artifactId>
                    </dependency>
                </dependencies>
            </project>"#,
        )
        .expect("valid");
        assert_eq!(root.name, "project");
        assert_eq!(root.text_of("groupId").as_deref(), Some("com.example"));
        let deps = root.child("dependencies").expect("dependencies");
        let dep = deps.children_named("dependency").next().expect("one dep");
        assert_eq!(dep.text_of("artifactId").as_deref(), Some("junit-jupiter"));
    }

    #[test]
    fn a_self_closing_element_has_no_text() {
        let root = parse("<project><packaging/></project>").expect("valid");
        assert_eq!(root.text_of("packaging"), None);
    }

    #[test]
    fn cdata_text_is_captured_like_ordinary_text() {
        let root = parse("<project><name><![CDATA[Example]]></name></project>").expect("valid");
        assert_eq!(root.text_of("name").as_deref(), Some("Example"));
    }

    #[test]
    fn malformed_xml_is_an_error_not_a_panic() {
        assert!(parse("<project><unclosed></project>").is_err());
    }
}
