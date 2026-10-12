//! Questions an agent asks the person (Claude's `AskUserQuestion`, Codex's
//! `item/tool/requestUserInput`), in one shape for every CLI.
//!
//! They travel the same queue as tool approvals (`anchorleg permission ask`), as a request whose
//! tool is [`TOOL`] and whose input is `{ "questions": [Question…] }`. The person's answer is
//! stored beside it as [`Answers`]: question id → the chosen labels, or the text they typed.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The tool name a question request is stored under.
pub const TOOL: &str = "AskUserQuestion";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Question {
    /// What the answer is keyed by: Codex's question id, or Claude's question text.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    pub question: String,
    /// Empty for a free-text question.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<Choice>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub multi_select: bool,
    /// The answer is a secret (Codex `isSecret`). anchorleg never passes one: secrets would end up
    /// in its database. The agent gets no answer to it and the person is told [`SECRET_NOTE`].
    #[serde(default, skip_serializing_if = "is_false")]
    pub secret: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

fn is_false(b: &bool) -> bool {
    !b
}

/// Shown where a secret question would be answered.
pub const SECRET_NOTE: &str = "anchorleg doesn't pass secrets (they would be stored on disk); \
     give the agent the value another way, e.g. an environment variable, and answer `-`";

/// Question id → answers (option labels, or one free-text answer).
pub type Answers = BTreeMap<String, Vec<String>>;

/// The questions in a stored request's input, if it is one.
pub fn from_input(input: &Value) -> Vec<Question> {
    input
        .get("questions")
        .cloned()
        .and_then(|q| serde_json::from_value(q).ok())
        .unwrap_or_default()
}

pub fn to_input(questions: &[Question]) -> Value {
    json!({ "questions": questions })
}

/// Claude's `AskUserQuestion` arguments (`{ questions: [{ question, header, options:
/// [{label, description}], multiSelect }] }`). Answers are keyed by the question text.
pub fn from_claude(input: &Value) -> Vec<Question> {
    input
        .get("questions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|q| {
            let text = q.get("question")?.as_str()?.to_owned();
            Some(Question {
                id: text.clone(),
                header: str_field(q, "header"),
                question: text,
                options: choices(q.get("options")),
                multi_select: q.get("multiSelect").and_then(Value::as_bool) == Some(true),
                secret: false,
            })
        })
        .collect()
}

/// Codex's `item/tool/requestUserInput` params (`questions: [{ id, header, question, options?:
/// [{label, description}], isSecret }]`).
pub fn from_codex(params: &Value) -> Vec<Question> {
    params
        .get("questions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|q| {
            Some(Question {
                id: q.get("id")?.as_str()?.to_owned(),
                header: str_field(q, "header"),
                question: q.get("question")?.as_str()?.to_owned(),
                options: choices(q.get("options")),
                multi_select: false,
                secret: q.get("isSecret").and_then(Value::as_bool) == Some(true),
            })
        })
        .collect()
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn choices(options: Option<&Value>) -> Vec<Choice> {
    options
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|o| {
            Some(Choice {
                label: o.get("label")?.as_str()?.to_owned(),
                description: str_field(o, "description"),
            })
        })
        .collect()
}

/// Codex's `ToolRequestUserInputResponse`: `{ answers: { <id>: { answers: [...] } } }`.
pub fn codex_response(answers: &Answers) -> Value {
    let map: serde_json::Map<String, Value> = answers
        .iter()
        .map(|(id, a)| (id.clone(), json!({ "answers": a })))
        .collect();
    json!({ "answers": map })
}

/// The answers to [`plan_question`]; anything else is the person's change to the plan.
pub const PLAN_APPROVE: &str = "Approve";
pub const PLAN_REVISE: &str = "Keep planning";

/// Asked when a plan-first run's plan is ready (D38). The plan itself is the agent's last
/// message, just above in the conversation.
pub fn plan_question() -> Question {
    Question {
        id: "plan".to_owned(),
        header: Some("Plan".to_owned()),
        question: "Approve the plan above? Or type what to change.".to_owned(),
        options: vec![
            Choice {
                label: PLAN_APPROVE.to_owned(),
                description: Some("carry it out now, in the same session".to_owned()),
            },
            Choice {
                label: PLAN_REVISE.to_owned(),
                description: Some("let the agent improve it first".to_owned()),
            },
        ],
        multi_select: false,
        secret: false,
    }
}

/// An MCP server's request for input (Codex `mcpServer/elicitation/request`), as questions: one
/// per form field, or, for a link to open, one question "Done" / "Decline". Fields that look like
/// credentials are marked secret, so their values are never stored or passed.
pub fn from_elicitation(params: &Value) -> Vec<Question> {
    let server = str_field(params, "serverName").unwrap_or_else(|| "MCP server".to_owned());
    let message = str_field(params, "message").unwrap_or_default();
    if params.get("mode").and_then(Value::as_str) == Some("url") {
        let url = str_field(params, "url").unwrap_or_default();
        return vec![Question {
            id: ELICIT_URL.to_owned(),
            header: Some(server),
            question: format!("{message} Open {url}, then answer Done."),
            options: ["Done", "Decline"]
                .map(|l| Choice {
                    label: l.to_owned(),
                    description: None,
                })
                .to_vec(),
            multi_select: false,
            secret: false,
        }];
    }
    let Some(props) = params
        .pointer("/requestedSchema/properties")
        .and_then(Value::as_object)
    else {
        return Vec::new();
    };
    if props.is_empty() {
        // Nothing to fill in: a yes/no confirmation.
        return vec![Question {
            id: ELICIT_CONFIRM.to_owned(),
            header: Some(server),
            question: message,
            options: ["Accept", "Decline"]
                .map(|l| Choice {
                    label: l.to_owned(),
                    description: None,
                })
                .to_vec(),
            multi_select: false,
            secret: false,
        }];
    }
    let single = props.len() == 1;
    props
        .iter()
        .map(|(name, field)| {
            let title = str_field(field, "title").unwrap_or_else(|| name.clone());
            let what = str_field(field, "description").unwrap_or_else(|| title.clone());
            let (options, multi_select) = field_options(field);
            Question {
                id: name.clone(),
                header: Some(format!("{server} · {title}")),
                question: if single || message.is_empty() {
                    format!("{message} {what}").trim().to_owned()
                } else {
                    format!("{message} ({what})")
                },
                options,
                multi_select,
                secret: looks_secret(name) || looks_secret(&title),
            }
        })
        .collect()
}

/// The id of the one question asked for a link elicitation, and for a form with no fields.
const ELICIT_URL: &str = "url";
const ELICIT_CONFIRM: &str = "confirm";

fn looks_secret(name: &str) -> bool {
    let n = name.to_ascii_lowercase().replace(['-', ' '], "_");
    [
        "password",
        "secret",
        "token",
        "api_key",
        "apikey",
        "credential",
        "private_key",
    ]
    .iter()
    .any(|w| n.contains(w))
}

/// A form field's choices: enum values (titled or not), or Yes / No for a boolean.
fn field_options(field: &Value) -> (Vec<Choice>, bool) {
    let choice = |label: &str| Choice {
        label: label.to_owned(),
        description: None,
    };
    let titled = |list: &Value| -> Vec<Choice> {
        list.as_array()
            .into_iter()
            .flatten()
            .filter_map(|o| {
                o.get("title")
                    .or_else(|| o.get("const"))
                    .and_then(Value::as_str)
                    .map(choice)
            })
            .collect()
    };
    let plain = |list: &Value, names: Option<&Value>| -> Vec<Choice> {
        let names: Vec<&str> = names
            .and_then(Value::as_array)
            .map(|n| n.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        list.as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .enumerate()
            .map(|(i, v)| choice(names.get(i).copied().unwrap_or(v)))
            .collect()
    };
    match field.get("type").and_then(Value::as_str) {
        Some("boolean") => (vec![choice("Yes"), choice("No")], false),
        Some("array") => {
            let items = field.get("items").unwrap_or(&Value::Null);
            let options = match (items.get("anyOf").or(items.get("oneOf")), items.get("enum")) {
                (Some(list), _) => titled(list),
                (None, Some(list)) => plain(list, None),
                _ => Vec::new(),
            };
            (options, true)
        }
        _ => match (field.get("oneOf"), field.get("enum")) {
            (Some(list), _) => (titled(list), false),
            (None, Some(list)) => (plain(list, field.get("enumNames")), false),
            _ => (Vec::new(), false),
        },
    }
}

/// The answer to an elicitation (`{action, content?}`): declined when the person skipped it
/// (or answered "Decline"), else every answered field in the type its schema asks for.
pub fn elicitation_response(params: &Value, answers: &Answers) -> Value {
    if params.get("mode").and_then(Value::as_str) == Some("url") {
        let done = answers
            .get(ELICIT_URL)
            .is_some_and(|a| a.iter().any(|x| x == "Done"));
        return json!({ "action": if done { "accept" } else { "decline" } });
    }
    if answers
        .get(ELICIT_CONFIRM)
        .is_some_and(|a| a.iter().any(|x| x == "Accept"))
    {
        return json!({ "action": "accept", "content": {} });
    }
    let questions = from_elicitation(params);
    let props = params.pointer("/requestedSchema/properties");
    let mut content = serde_json::Map::new();
    for q in &questions {
        let (Some(answer), Some(field)) = (answers.get(&q.id), props.and_then(|p| p.get(&q.id)))
        else {
            continue;
        };
        if q.secret {
            continue;
        }
        if let Some(v) = typed_value(field, answer) {
            content.insert(q.id.clone(), v);
        }
    }
    if content.is_empty() {
        return json!({ "action": "decline" });
    }
    json!({ "action": "accept", "content": content })
}

/// `answer` (labels or typed text) as the JSON value `field` asks for.
fn typed_value(field: &Value, answer: &[String]) -> Option<Value> {
    // A label shown for a titled or named option → the value behind it.
    let value_of = |label: &str, field: &Value| -> String {
        if let Some(list) = field
            .get("oneOf")
            .or(field.get("anyOf"))
            .and_then(Value::as_array)
            && let Some(o) = list
                .iter()
                .find(|o| o.get("title").and_then(Value::as_str) == Some(label))
        {
            return o
                .get("const")
                .and_then(Value::as_str)
                .unwrap_or(label)
                .to_owned();
        }
        if let (Some(values), Some(names)) = (
            field.get("enum").and_then(Value::as_array),
            field.get("enumNames").and_then(Value::as_array),
        ) && let Some(i) = names.iter().position(|n| n.as_str() == Some(label))
        {
            return values
                .get(i)
                .and_then(Value::as_str)
                .unwrap_or(label)
                .to_owned();
        }
        label.to_owned()
    };
    let first = answer.first()?.trim();
    match field.get("type").and_then(Value::as_str) {
        Some("boolean") => match first.to_ascii_lowercase().as_str() {
            "yes" | "true" | "y" => Some(json!(true)),
            "no" | "false" | "n" => Some(json!(false)),
            _ => None,
        },
        Some("integer") => first.parse::<i64>().ok().map(Value::from),
        Some("number") => first.parse::<f64>().ok().map(Value::from),
        Some("array") => {
            let items = field.get("items").unwrap_or(&Value::Null);
            Some(Value::from(
                answer
                    .iter()
                    .map(|a| value_of(a, items))
                    .collect::<Vec<_>>(),
            ))
        }
        _ => Some(Value::from(value_of(first, field))),
    }
}

/// Read one typed answer to `q`: an option number (`2`, or `1,3` for multi-select), an option
/// label (any case), or free text.
pub fn parse_answer(q: &Question, text: &str) -> Vec<String> {
    let text = text.trim();
    if q.options.is_empty() {
        return vec![text.to_owned()];
    }
    let parts: Vec<&str> = if q.multi_select {
        text.split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect()
    } else {
        vec![text]
    };
    let picked: Option<Vec<String>> = parts
        .iter()
        .map(|p| {
            p.parse::<usize>()
                .ok()
                .and_then(|n| n.checked_sub(1))
                .and_then(|i| q.options.get(i))
                .or_else(|| q.options.iter().find(|o| o.label.eq_ignore_ascii_case(p)))
                .map(|o| o.label.clone())
        })
        .collect();
    match picked {
        Some(labels) if !labels.is_empty() => labels,
        // Not an option: the person's own words ("Other").
        _ => vec![text.to_owned()],
    }
}

/// One line per question and answer, for logs and the conversation. Secret answers are hidden.
pub fn summary(questions: &[Question], answers: &Answers) -> String {
    questions
        .iter()
        .map(|q| {
            let a = match answers.get(&q.id) {
                _ if q.secret => "(secret, not passed)".to_owned(),
                Some(a) => a.join(", "),
                None => "(no answer)".to_owned(),
            };
            format!("{} → {a}", q.header.as_deref().unwrap_or(&q.question))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_and_codex_shapes() {
        let claude = json!({ "questions": [{
            "question": "Which test runner?", "header": "Runner", "multiSelect": false,
            "options": [{ "label": "Vitest", "description": "fast" }, { "label": "Jest", "description": "" }]
        }]});
        let q = from_claude(&claude);
        assert_eq!(q[0].id, "Which test runner?");
        assert_eq!(q[0].options[1].label, "Jest");
        assert_eq!(q[0].options[1].description, None);

        let codex = json!({ "threadId": "t", "turnId": "u", "itemId": "i", "isBlocking": true,
            "questions": [{ "id": "q1", "header": "Token", "question": "API token?", "isSecret": true }] });
        let q = from_codex(&codex);
        assert_eq!((q[0].id.as_str(), q[0].secret), ("q1", true));
        assert!(q[0].options.is_empty());

        let answers: Answers = [("q1".to_owned(), vec!["abc".to_owned()])].into();
        assert_eq!(
            codex_response(&answers),
            json!({ "answers": { "q1": { "answers": ["abc"] } } })
        );
        assert_eq!(summary(&q, &answers), "Token → (secret, not passed)");
        // Stored and read back unchanged.
        assert_eq!(from_input(&to_input(&q)), q);
    }

    #[test]
    fn mcp_forms_and_links() {
        let form = json!({ "serverName": "jira", "threadId": "t", "mode": "form",
        "message": "Create the ticket?",
        "requestedSchema": { "type": "object", "properties": {
            "project": { "type": "string", "title": "Project",
                "oneOf": [{ "const": "ENG", "title": "Engineering" }, { "const": "OPS", "title": "Operations" }] },
            "points": { "type": "integer", "title": "Story points" },
            "notify": { "type": "boolean" },
            "api_token": { "type": "string" }
        }}});
        let qs = from_elicitation(&form);
        let q = |id: &str| qs.iter().find(|q| q.id == id).unwrap();
        assert_eq!(q("project").options[1].label, "Operations");
        assert_eq!(q("notify").options.len(), 2);
        assert!(q("api_token").secret);
        assert_eq!(q("points").header.as_deref(), Some("jira · Story points"));

        let answers: Answers = [
            ("project".to_owned(), parse_answer(q("project"), "2")),
            ("points".to_owned(), vec!["3".to_owned()]),
            ("notify".to_owned(), parse_answer(q("notify"), "1")),
            ("api_token".to_owned(), vec!["leak".to_owned()]),
        ]
        .into();
        assert_eq!(
            elicitation_response(&form, &answers),
            json!({ "action": "accept", "content": { "project": "OPS", "points": 3, "notify": true } })
        );
        assert_eq!(
            elicitation_response(&form, &Answers::new()),
            json!({ "action": "decline" })
        );

        let link = json!({ "serverName": "gh", "threadId": "t", "mode": "url", "elicitationId": "e",
            "message": "Sign in to GitHub.", "url": "https://example.com/login" });
        let qs = from_elicitation(&link);
        assert!(qs[0].question.contains("https://example.com/login"));
        let done: Answers = [("url".to_owned(), parse_answer(&qs[0], "1"))].into();
        assert_eq!(
            elicitation_response(&link, &done),
            json!({ "action": "accept" })
        );
    }

    #[test]
    fn typed_answers() {
        let q = Question {
            id: "x".into(),
            header: None,
            question: "Pick".into(),
            options: ["Vitest", "Jest", "Mocha"]
                .map(|l| Choice {
                    label: l.into(),
                    description: None,
                })
                .to_vec(),
            multi_select: false,
            secret: false,
        };
        assert_eq!(parse_answer(&q, "2"), ["Jest"]);
        assert_eq!(parse_answer(&q, "vitest"), ["Vitest"]);
        assert_eq!(
            parse_answer(&q, "neither, use node:test"),
            ["neither, use node:test"]
        );
        let multi = Question {
            multi_select: true,
            ..q
        };
        assert_eq!(parse_answer(&multi, "1, 3"), ["Vitest", "Mocha"]);
    }
}
