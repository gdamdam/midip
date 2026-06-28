use crate::pattern::model::LaneKind;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeviceProfile {
    pub id: &'static str,
    pub label: &'static str,
    pub port_match: &'static str,
    pub kind: LaneKind,
    pub channel: u8,
    pub root_note: u8,
    pub gate_fraction: f32,
    pub drum_gate_fraction: f32,
    pub send_clock: bool,
}

pub const T8_DRUMS: DeviceProfile = DeviceProfile {
    id: "t8-drums", label: "T-8 DRUM", port_match: "T-8", kind: LaneKind::Drums,
    channel: 9, root_note: 0, gate_fraction: 0.0, drum_gate_fraction: 0.1, send_clock: true,
};
pub const T8_BASS: DeviceProfile = DeviceProfile {
    id: "t8-bass", label: "T-8 BASS", port_match: "T-8", kind: LaneKind::Melodic,
    channel: 1, root_note: 45, gate_fraction: 0.5, drum_gate_fraction: 0.0, send_clock: true,
};
pub const S1: DeviceProfile = DeviceProfile {
    id: "s1", label: "S-1 SYNTH", port_match: "S-1", kind: LaneKind::Melodic,
    channel: 0, root_note: 45, gate_fraction: 0.9, drum_gate_fraction: 0.0, send_clock: true,
};

pub fn default_profiles() -> [DeviceProfile; 3] { [T8_DRUMS, T8_BASS, S1] }
