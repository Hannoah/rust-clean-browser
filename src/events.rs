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

pub fn winit_ime_to_blitz(event: Ime) -> BlitzImeEvent {
    match event {
        Ime::Enabled => BlitzImeEvent::Enabled,
        Ime::Disabled => BlitzImeEvent::Disabled,
        Ime::Preedit(text, cursor) => BlitzImeEvent::Preedit(text, cursor),
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
