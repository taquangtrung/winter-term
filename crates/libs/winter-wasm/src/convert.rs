//! The extension API's types, carried across to a component and back.

use winter_ext_api::{
    CompletionItem, CompletionRequest, Completions, Diagnostic, Document, Event, InlineAnswer,
    InlineEdit, InlineKind, InlineOffer, InlineRequest, Install, KeyName, KeyPress, Line,
    PickerEvent, PickerRow, PickerSpec, Platform, ServerSaid, ServerSpec, SettingValue, Severity,
    Styled, TextPosition, TextRange, Tone,
};

use super::bindings::winter::extension::types;

// ========================================================================
// Functions
// ========================================================================

pub(super) fn to_completions(completions: types::Completions) -> Completions {
    let items = completions
        .items
        .into_iter()
        .map(|item| {
            let offered = CompletionItem::new(item.label, item.insert);
            match item.detail {
                Some(detail) => offered.with_detail(detail),
                None => offered,
            }
        })
        .collect();
    Completions::new(completions.start, items)
}

pub(super) fn to_diagnostic(diagnostic: types::Diagnostic) -> Diagnostic {
    let severity = match diagnostic.severity {
        types::Severity::Error => Severity::Error,
        types::Severity::Hint => Severity::Hint,
        types::Severity::Info => Severity::Info,
        types::Severity::Warning => Severity::Warning,
    };
    let columns = diagnostic.start..diagnostic.end;
    Diagnostic::new(severity, diagnostic.message, diagnostic.line, columns)
}

pub(super) fn to_line(line: types::Line) -> Line {
    line.into_iter()
        .map(|styled| {
            let tone = match styled.tone {
                types::Tone::Accent => Tone::Accent,
                types::Tone::Dim => Tone::Dim,
                types::Tone::Error => Tone::Error,
                types::Tone::Heading => Tone::Heading,
                types::Tone::Info => Tone::Info,
                types::Tone::Normal => Tone::Normal,
                types::Tone::Warning => Tone::Warning,
            };
            Styled::new(tone, styled.text)
        })
        .collect()
}

pub(super) fn to_picker_spec(spec: types::PickerSpec) -> PickerSpec {
    let rows = spec.rows.into_iter().map(to_picker_row).collect();
    let mut made = PickerSpec::new(spec.title).with_rows(rows);
    if spec.streamed {
        made = made.streamed();
    }
    match spec.hears_query {
        true => made.hearing_query(),
        false => made,
    }
}

pub(super) fn to_picker_row(row: types::PickerRow) -> PickerRow {
    let mut made = PickerRow::new(row.label);
    if let Some(description) = row.description {
        made = made.with_description(description);
    }
    if let Some(detail) = row.detail {
        made = made.with_detail(detail);
    }
    if let Some(file) = row.file {
        made = made.with_file(file);
    }
    if let Some(preview) = row.preview {
        made = made.with_preview(preview);
    }
    row.actions.into_iter().fold(made, |made, action| {
        made.with_action(action.id, action.title)
    })
}

/// What happened in a picker, as the guest hears it.
pub(super) fn to_wit_picker_event(event: PickerEvent) -> Option<types::PickerEvent> {
    match event {
        PickerEvent::Chosen(choice) => Some(types::PickerEvent::Chosen(types::PickerChoice {
            action: choice.action,
            row: u32::try_from(choice.row).unwrap_or(u32::MAX),
        })),
        PickerEvent::Closed => Some(types::PickerEvent::Closed),
        PickerEvent::Query(query) => Some(types::PickerEvent::Query(query)),
        // An event added after this crate was built: not one the guest knows.
        _ => None,
    }
}

pub(super) fn to_wit_document(document: &Document) -> types::Document {
    types::Document {
        path: document.path.to_string_lossy().into_owned(),
        text: document.text.clone(),
    }
}

/// `event` as a component hears of it, or `None` for one added to the API
/// after the contract was written.
pub(super) fn to_wit_event(event: &Event) -> Option<types::Event> {
    let path = |path: &std::path::Path| path.to_string_lossy().into_owned();
    Some(match event {
        Event::Configured(settings) => types::Event::Configured(
            settings
                .iter()
                .filter_map(|(name, value)| {
                    Some(types::SettingEntry {
                        name: name.to_string(),
                        value: to_wit_setting_value(value)?,
                    })
                })
                .collect(),
        ),
        Event::Server(event) => types::Event::Server(types::ServerEvent {
            said: to_wit_server_said(&event.said)?,
            server: event.server.clone(),
        }),
        Event::Changed(changed) => types::Event::Changed(path(changed)),
        Event::Closed(closed) => types::Event::Closed(path(closed)),
        Event::Focused(front) => types::Event::Focused(front.as_deref().map(path)),
        Event::Opened(opened) => types::Event::Opened(path(opened)),
        Event::Saved(saved) => types::Event::Saved(path(saved)),
        _ => return None,
    })
}

/// `request` as a component hears it.
pub(super) fn to_wit_inline_request(request: &InlineRequest) -> types::InlineRequest {
    types::InlineRequest {
        at: types::TextPosition {
            column: request.at.column,
            line: request.at.line,
        },
        insert_spaces: request.insert_spaces,
        invoked: request.invoked,
        kind: to_wit_inline_kind(request.kind),
        path: request.path.to_string_lossy().into_owned(),
        tab_size: request.tab_size,
        ticket: request.ticket,
        uri: request.uri.clone(),
        version: request.version,
    }
}

/// `kind` as a component hears it; a kind added after the contract was
/// written is heard as ghost text.
pub(super) fn to_wit_inline_kind(kind: InlineKind) -> types::InlineKind {
    match kind {
        InlineKind::Edit => types::InlineKind::Edit,
        _ => types::InlineKind::Ghost,
    }
}

pub(super) fn to_inline_offer(offer: types::InlineOffer) -> InlineOffer {
    InlineOffer::new(offer.ghost, offer.edit)
}

pub(super) fn to_inline_answer(answer: types::InlineAnswer) -> InlineAnswer {
    let edit = |edit: types::InlineEdit| {
        let at = |at: types::TextPosition| TextPosition::new(at.line, at.column);
        let range = TextRange::new(at(edit.range.start), at(edit.range.end));
        InlineEdit::new(range, edit.text, edit.receipt)
    };
    match answer {
        types::InlineAnswer::Edit(made) => InlineAnswer::Edit(edit(made)),
        types::InlineAnswer::Ghost(made) => {
            InlineAnswer::Ghost(made.into_iter().map(edit).collect())
        }
        types::InlineAnswer::Nothing => InlineAnswer::Nothing,
    }
}

/// A server from the guest's manifest.
pub(super) fn to_server_spec(server: types::ServerSpec) -> ServerSpec {
    let mut spec = ServerSpec::new(server.name, server.program);
    spec.args = server.args;
    spec.init_options = server.init_options;
    server.install.into_iter().fold(spec, |spec, install| {
        let platform = match install.platform {
            types::Platform::LinuxArm64 => Platform::LinuxArm64,
            types::Platform::LinuxX64 => Platform::LinuxX64,
            types::Platform::MacArm64 => Platform::MacArm64,
            types::Platform::MacX64 => Platform::MacX64,
            types::Platform::WindowsArm64 => Platform::WindowsArm64,
            types::Platform::WindowsX64 => Platform::WindowsX64,
        };
        spec.with_install(Install::new(platform, install.url, install.member))
    })
}

/// What a server said, as a component hears of it, or `None` for what was
/// added to the API after the contract was written.
fn to_wit_server_said(said: &ServerSaid) -> Option<types::ServerSaid> {
    Some(match said {
        ServerSaid::Answer(answer) => types::ServerSaid::Answer(types::ServerAnswer {
            result: answer.result.clone(),
            tag: answer.tag,
        }),
        ServerSaid::Exited(why) => types::ServerSaid::Exited(why.clone()),
        ServerSaid::Message(message) => types::ServerSaid::Message(message.clone()),
        ServerSaid::OpenUrl(url) => types::ServerSaid::OpenUrl(url.clone()),
        ServerSaid::Ready => types::ServerSaid::Ready,
        ServerSaid::Status(status) => types::ServerSaid::Status(types::ServerStatus {
            error: status.error,
            message: status.message.clone(),
        }),
        _ => return None,
    })
}

/// A setting's value from the guest's manifest.
pub(super) fn to_setting_value(value: types::SettingValue) -> SettingValue {
    match value {
        types::SettingValue::Number(number) => SettingValue::Number(number),
        types::SettingValue::Switch(on) => SettingValue::Switch(on),
        types::SettingValue::Text(text) => SettingValue::Text(text),
    }
}

/// `value` as a component hears of it, or `None` for a kind added to the
/// API after the contract was written.
fn to_wit_setting_value(value: &SettingValue) -> Option<types::SettingValue> {
    Some(match value {
        SettingValue::Number(number) => types::SettingValue::Number(*number),
        SettingValue::Switch(on) => types::SettingValue::Switch(*on),
        SettingValue::Text(text) => types::SettingValue::Text(text.clone()),
        _ => return None,
    })
}

/// `key` as a component hears of it, or `None` for one added to the API
/// after the contract was written.
pub(super) fn to_wit_key(key: &KeyPress) -> Option<types::KeyPress> {
    let name = match key.key {
        KeyName::Backspace => types::KeyName::Backspace,
        KeyName::Char(c) => types::KeyName::Char(c),
        KeyName::Delete => types::KeyName::Delete,
        KeyName::Down => types::KeyName::Down,
        KeyName::End => types::KeyName::End,
        KeyName::Enter => types::KeyName::Enter,
        KeyName::Escape => types::KeyName::Escape,
        KeyName::F(number) => types::KeyName::F(number),
        KeyName::Home => types::KeyName::Home,
        KeyName::Left => types::KeyName::Left,
        KeyName::PageDown => types::KeyName::PageDown,
        KeyName::PageUp => types::KeyName::PageUp,
        KeyName::Right => types::KeyName::Right,
        KeyName::Tab => types::KeyName::Tab,
        KeyName::Up => types::KeyName::Up,
        _ => return None,
    };
    Some(types::KeyPress {
        alt: key.alt,
        ctrl: key.ctrl,
        key: name,
        shift: key.shift,
    })
}

pub(super) fn to_wit_request(request: &CompletionRequest) -> types::CompletionRequest {
    types::CompletionRequest {
        column: request.column,
        language: request.language.to_string(),
        line: request.line.to_string(),
        path: request.path.to_string_lossy().into_owned(),
        row: request.row,
    }
}
