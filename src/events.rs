use blitz_traits::events::{BlitzImeEvent, BlitzKeyEvent, BlitzPointerId, KeyState, PointerDetails};
use blitz_traits::shell::ColorScheme;
use keyboard_types::{Code, Key, Modifiers, NamedKey};
use winit::event::{ButtonSource, ElementState, Ime, KeyEvent as WinitKeyEvent, PointerKind, PointerSource};
use winit::keyboard::{Key as WinitKey, ModifiersState as WinitModifiers, PhysicalKey as WinitPhysicalKey};
use winit::window::Theme;

pub fn theme_to_color_scheme(theme: Theme) -> ColorScheme {
    match theme {
        Theme::Light => ColorScheme::Light,
        Theme::Dark => ColorScheme::Dark,
    }
}

fn is_hangul_char(c: char) -> bool {
    matches!(
        c,
        '\u{1100}'..='\u{11FF}'
            | '\u{3130}'..='\u{318F}'
            | '\u{A960}'..='\u{A97F}'
            | '\u{AC00}'..='\u{D7AF}'
            | '\u{D7B0}'..='\u{D7FF}'
    )
}

pub fn winit_ime_to_blitz(event: Ime) -> BlitzImeEvent {
    match event {
        Ime::Enabled => BlitzImeEvent::Enabled,
        Ime::Disabled => BlitzImeEvent::Disabled,
        Ime::Preedit(text, mut cursor) => {
            // Windows Hangul IME reports `GCS_CURSORPOS = 0` with `ATTR_INPUT`, which makes
            // Parley place the caret at the left edge of the composing syllable instead of its end.
            if !text.is_empty() && text.chars().all(is_hangul_char) && matches!(cursor, None | Some((0, 0))) {
                cursor = Some((text.len(), text.len()));
            }
            BlitzImeEvent::Preedit(text, cursor)
        }
        Ime::Commit(text) => BlitzImeEvent::Commit(text),
        Ime::DeleteSurrounding {
            before_bytes,
            after_bytes,
        } => BlitzImeEvent::DeleteSurrounding {
            before_bytes,
            after_bytes,
        },
        _ => BlitzImeEvent::Disabled,
    }
}

pub fn winit_key_event_to_blitz(event: &WinitKeyEvent, mods: WinitModifiers) -> BlitzKeyEvent {
    BlitzKeyEvent {
        key: winit_key_to_kbt_key(&event.logical_key),
        code: winit_physical_key_to_kbt_code(&event.physical_key),
        modifiers: winit_modifiers_to_kbt_modifiers(mods),
        location: event.location,
        is_auto_repeating: event.repeat,
        is_composing: false,
        state: match event.state {
            ElementState::Pressed => KeyState::Pressed,
            ElementState::Released => KeyState::Released,
        },
        text: event.text.clone(),
    }
}

pub fn pointer_source_to_blitz(source: &PointerSource) -> BlitzPointerId {
    match source {
        PointerSource::Mouse => BlitzPointerId::Mouse,
        PointerSource::Touch { finger_id, .. } => BlitzPointerId::Finger(finger_id.into_raw() as u64),
        PointerSource::TabletTool { .. } => BlitzPointerId::Pen,
        _ => BlitzPointerId::Mouse,
    }
}

pub fn pointer_kind_to_blitz(kind: &PointerKind) -> BlitzPointerId {
    match kind {
        PointerKind::Mouse => BlitzPointerId::Mouse,
        PointerKind::Touch(finger_id) => BlitzPointerId::Finger(finger_id.into_raw() as u64),
        PointerKind::TabletTool(_) => BlitzPointerId::Pen,
        _ => BlitzPointerId::Mouse,
    }
}

pub fn button_source_to_blitz(source: &ButtonSource) -> BlitzPointerId {
    match source {
        ButtonSource::Mouse(_) => BlitzPointerId::Mouse,
        ButtonSource::TabletTool { .. } => BlitzPointerId::Pen,
        ButtonSource::Touch { finger_id, .. } => BlitzPointerId::Finger(finger_id.into_raw() as u64),
        _ => BlitzPointerId::Mouse,
    }
}

pub fn pointer_source_to_blitz_details(source: &PointerSource) -> PointerDetails {
    match source {
        PointerSource::Touch { force, .. } => PointerDetails {
            pressure: force.map(|f| f.normalized(None)).unwrap_or(0.0),
            ..PointerDetails::default()
        },
        PointerSource::TabletTool { data, .. } => PointerDetails {
            pressure: data.force.map(|f| f.normalized(data.angle)).unwrap_or(0.0),
            tangential_pressure: data.tangential_force.unwrap_or(0.0),
            tilt_x: data.tilt.map(|t| t.x).unwrap_or(0),
            tilt_y: data.tilt.map(|t| t.y).unwrap_or(0),
            twist: data.twist.unwrap_or(0),
            altitude: data.angle.map(|a| a.altitude).unwrap_or(0.0),
            azimuth: data.angle.map(|a| a.azimuth).unwrap_or(0.0),
        },
        _ => PointerDetails::default(),
    }
}

pub fn winit_modifiers_to_kbt_modifiers(winit_modifiers: WinitModifiers) -> Modifiers {
    let mut modifiers = Modifiers::default();
    if winit_modifiers.control_key() {
        modifiers.insert(Modifiers::CONTROL);
    }
    if winit_modifiers.alt_key() {
        modifiers.insert(Modifiers::ALT);
    }
    if winit_modifiers.shift_key() {
        modifiers.insert(Modifiers::SHIFT);
    }
    if winit_modifiers.meta_key() {
        modifiers.insert(Modifiers::META);
    }
    modifiers
}

pub fn winit_physical_key_to_kbt_code(physical_key: &WinitPhysicalKey) -> Code {
    match physical_key {
        WinitPhysicalKey::Code(code) => *code,
        WinitPhysicalKey::Unidentified(_) => Code::Unidentified,
    }
}

pub fn winit_key_to_kbt_key(winit_key: &WinitKey) -> Key {
    match winit_key {
        WinitKey::Named(named_key) => Key::Named(*named_key),
        WinitKey::Character(c) => Key::Character(c.to_string()),
        WinitKey::Unidentified(_) => Key::Named(NamedKey::Unidentified),
        WinitKey::Dead(_) => Key::Named(NamedKey::Dead),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blitz_dom::{Document, DocumentConfig, EventDriver, EventHandler, NodeId, StyleThreading};
    use blitz_traits::events::{DomEvent, EventState, UiEvent};
    use blitz_traits::shell::{ColorScheme, Viewport};

    struct NoopHandler;

    impl EventHandler for NoopHandler {
        fn handle_event(&mut self, _: &[NodeId], _: &mut DomEvent, _: &mut dyn Document, _: &mut EventState) {}
    }

    #[test]
    fn hangul_preedit_moves_caret_to_end() {
        assert_eq!(
            winit_ime_to_blitz(Ime::Preedit("ㄴ".into(), Some((0, 0)))),
            BlitzImeEvent::Preedit("ㄴ".into(), Some(("ㄴ".len(), "ㄴ".len())))
        );
        assert_eq!(
            winit_ime_to_blitz(Ime::Preedit("한".into(), None)),
            BlitzImeEvent::Preedit("한".into(), Some(("한".len(), "한".len())))
        );
        assert_eq!(
            winit_ime_to_blitz(Ime::Preedit(String::new(), None)),
            BlitzImeEvent::Preedit(String::new(), None)
        );
        assert_eq!(
            winit_ime_to_blitz(Ime::Preedit("あ".into(), Some((0, 0)))),
            BlitzImeEvent::Preedit("あ".into(), Some((0, 0)))
        );
        assert_eq!(
            winit_ime_to_blitz(Ime::Preedit("abc".into(), Some((0, 0)))),
            BlitzImeEvent::Preedit("abc".into(), Some((0, 0)))
        );
        assert_eq!(
            winit_ime_to_blitz(Ime::Preedit("한".into(), Some((0, 3)))),
            BlitzImeEvent::Preedit("한".into(), Some((0, 3)))
        );
    }

    #[test]
    fn hangul_preedit_caret_sits_after_composing_syllable_in_blitz() {
        let html =
            r#"<!DOCTYPE html><html><body><input id="t" type="text" style="width:240px;height:28px;" /></body></html>"#;
        let mut doc = blitz_html::HtmlDocument::from_html(
            html,
            DocumentConfig {
                viewport: Some(Viewport::new(320, 80, 1.0, ColorScheme::Light)),
                style_threading: StyleThreading::Sequential,
                ..Default::default()
            },
        );
        doc.resolve(0.0);
        let id = doc.get_element_by_id("t").expect("input #t");
        doc.set_focus_to(id);

        EventDriver::new(&mut doc, NoopHandler)
            .handle_ui_event(UiEvent::Ime(winit_ime_to_blitz(Ime::Commit("안".into()))));
        EventDriver::new(&mut doc, NoopHandler).handle_ui_event(UiEvent::Ime(winit_ime_to_blitz(Ime::Preedit(
            "ㄴ".into(),
            Some((0, 0)),
        ))));

        let ti = doc
            .get_node(id)
            .unwrap()
            .element_data()
            .unwrap()
            .text_input_data()
            .unwrap();
        assert_eq!(ti.editor.raw_text(), "안ㄴ");
        assert_eq!(ti.editor.raw_selection().focus().index(), "안ㄴ".len());
    }
}
