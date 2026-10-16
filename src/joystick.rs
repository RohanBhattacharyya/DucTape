//! Joystick slots, physical controllers (via gilrs), and the virtual joystick.
//!
//! Robot code sees six joystick ports. Each port holds at most one device. Physical controllers
//! are bound to the first free port when they're plugged in; the virtual joystick starts
//! unbound and is bound on demand (by the UI, or automatically when an MCP client drives it).

use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use ds::JoystickValue;
use gilrs::{EventType, GamepadId, Gilrs};
use uuid::Uuid;

use crate::protocol::{JoystickDevice, JoystickSlot, VirtualJoystickState};

mod mapping;

pub const SLOTS: usize = 6;
pub const VIRTUAL_ID: Uuid = Uuid::nil();
pub const VIRTUAL_NAME: &str = "Virtual Joystick";

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Virtual,
    Physical(GamepadId),
}

#[derive(Debug, Clone)]
struct Device {
    id: Uuid,
    name: String,
    kind: Kind,
    values: Vec<JoystickValue>,
}

impl Device {
    fn info(&self) -> JoystickDevice {
        JoystickDevice {
            id: self.id,
            name: self.name.clone(),
            is_virtual: self.kind == Kind::Virtual,
        }
    }
}

struct Inner {
    slots: [Option<Uuid>; SLOTS],
    devices: Vec<Device>,
    virtual_state: VirtualJoystickState,
}

/// Returned when a physical controller is unplugged
pub struct Removed {
    pub name: String,
    /// Whether it was bound to a slot, i.e. robot code was reading it
    pub was_bound: bool,
}

pub struct Joysticks {
    inner: Mutex<Inner>,
}

impl Default for Joysticks {
    fn default() -> Self {
        Joysticks {
            inner: Mutex::new(Inner {
                slots: [None; SLOTS],
                devices: vec![Device {
                    id: VIRTUAL_ID,
                    name: VIRTUAL_NAME.into(),
                    kind: Kind::Virtual,
                    values: Vec::new(),
                }],
                virtual_state: neutral(),
            }),
        }
    }
}

fn neutral() -> VirtualJoystickState {
    VirtualJoystickState {
        axes: [0.0; 6],
        buttons: [false; 10],
        pov: -1,
    }
}

impl Inner {
    fn slot_of(&self, id: Uuid) -> Option<usize> {
        self.slots.iter().position(|s| *s == Some(id))
    }

    fn first_free(&self) -> Option<usize> {
        self.slots.iter().position(Option::is_none)
    }
}

impl Joysticks {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Values for each slot, as the driver station expects them. Called every 20ms.
    pub fn supply(&self) -> Vec<Vec<JoystickValue>> {
        let inner = self.lock();
        let Some(last) = inner.slots.iter().rposition(Option::is_some) else {
            return Vec::new();
        };
        inner.slots[..=last]
            .iter()
            .map(|slot| {
                let Some(id) = slot else { return Vec::new() };
                match inner.devices.iter().find(|d| d.id == *id) {
                    Some(d) if d.kind == Kind::Virtual => virtual_values(&inner.virtual_state),
                    Some(d) => d.values.clone(),
                    None => Vec::new(),
                }
            })
            .collect()
    }

    pub fn slots(&self) -> (Vec<JoystickSlot>, Vec<JoystickDevice>) {
        let inner = self.lock();
        let slots = inner
            .slots
            .iter()
            .enumerate()
            .map(|(slot, id)| JoystickSlot {
                slot,
                device: id
                    .and_then(|id| inner.devices.iter().find(|d| d.id == id).map(Device::info)),
            })
            .collect();
        let unbound = inner
            .devices
            .iter()
            .filter(|d| inner.slot_of(d.id).is_none())
            .map(Device::info)
            .collect();
        (slots, unbound)
    }

    pub fn has_physical(&self) -> bool {
        self.lock()
            .devices
            .iter()
            .any(|d| matches!(d.kind, Kind::Physical(_)))
    }

    pub fn virtual_state(&self) -> VirtualJoystickState {
        self.lock().virtual_state.clone()
    }

    pub fn virtual_slot(&self) -> Option<usize> {
        self.lock().slot_of(VIRTUAL_ID)
    }

    /// Changes the virtual joystick's values
    pub fn update_virtual(&self, update: impl FnOnce(&mut VirtualJoystickState)) {
        update(&mut self.lock().virtual_state);
    }

    pub fn reset_virtual(&self) {
        self.lock().virtual_state = neutral();
    }

    fn add_physical(&self, gid: GamepadId, name: String) -> Uuid {
        let mut inner = self.lock();
        let id = Uuid::new_v4();
        inner.devices.push(Device {
            id,
            name,
            kind: Kind::Physical(gid),
            values: Vec::new(),
        });
        if let Some(slot) = inner.first_free() {
            inner.slots[slot] = Some(id);
        }
        id
    }

    fn remove_physical(&self, gid: GamepadId) -> Option<Removed> {
        let mut inner = self.lock();
        let idx = inner
            .devices
            .iter()
            .position(|d| d.kind == Kind::Physical(gid))?;
        let device = inner.devices.remove(idx);
        let slot = inner.slot_of(device.id);
        if let Some(slot) = slot {
            inner.slots[slot] = None;
        }
        Some(Removed {
            name: device.name,
            was_bound: slot.is_some(),
        })
    }

    fn set_values(&self, gid: GamepadId, values: Vec<JoystickValue>) {
        if let Some(d) = self
            .lock()
            .devices
            .iter_mut()
            .find(|d| d.kind == Kind::Physical(gid))
        {
            d.values = values;
        }
    }

    /// Moves the device in slot `from` to slot `to`, shifting the slots in between
    pub fn move_slot(&self, from: usize, to: usize) -> Result<(), String> {
        if from >= SLOTS || to >= SLOTS {
            return Err(format!("joystick slots are 0-{}", SLOTS - 1));
        }
        let mut inner = self.lock();
        let device = inner.slots[from];
        if from < to {
            inner.slots.copy_within(from + 1..=to, from);
        } else {
            inner.slots.copy_within(to..from, to + 1);
        }
        inner.slots[to] = device;
        Ok(())
    }

    /// Binds a device to `slot` (or the first free slot). A device already in that slot is
    /// swapped into the bound device's previous slot, or unbound.
    pub fn bind(&self, id: Uuid, slot: Option<usize>) -> Result<usize, String> {
        let mut inner = self.lock();
        if !inner.devices.iter().any(|d| d.id == id) {
            return Err(format!("no joystick with id {id}"));
        }
        let previous = inner.slot_of(id);
        let target = match slot {
            Some(slot) if slot < SLOTS => slot,
            Some(_) => return Err(format!("joystick slots are 0-{}", SLOTS - 1)),
            None => match previous.or_else(|| inner.first_free()) {
                Some(slot) => slot,
                None => return Err("all joystick slots are in use".into()),
            },
        };
        let displaced = inner.slots[target];
        if let Some(prev) = previous {
            inner.slots[prev] = displaced.filter(|d| *d != id);
        }
        inner.slots[target] = Some(id);
        Ok(target)
    }

    pub fn unbind(&self, id: Uuid) -> Result<(), String> {
        let mut inner = self.lock();
        match inner.slot_of(id) {
            Some(slot) => {
                inner.slots[slot] = None;
                Ok(())
            }
            None if inner.devices.iter().any(|d| d.id == id) => Ok(()),
            None => Err(format!("no joystick with id {id}")),
        }
    }
}

fn virtual_values(state: &VirtualJoystickState) -> Vec<JoystickValue> {
    let axes = state
        .axes
        .iter()
        .enumerate()
        .map(|(id, &value)| JoystickValue::Axis {
            id: id as u8,
            value,
        });
    let buttons = state
        .buttons
        .iter()
        .enumerate()
        .map(|(i, &pressed)| JoystickValue::Button {
            id: i as u8 + 1,
            pressed,
        });
    axes.chain(buttons)
        .chain(std::iter::once(JoystickValue::POV {
            id: 0,
            angle: state.pov,
        }))
        .collect()
}

/// Events the input thread reports back
pub enum InputEvent {
    Connected(String),
    Removed(Removed),
}

/// Polls physical controllers on a dedicated thread (gilrs isn't `Send` on every platform).
pub fn spawn_input_thread(
    joysticks: std::sync::Arc<Joysticks>,
    on_event: impl Fn(InputEvent) + Send + 'static,
) {
    let spawned = thread::Builder::new()
        .name("joysticks".into())
        .spawn(move || {
            let mut gil = match Gilrs::new() {
                Ok(gil) => gil,
                Err(gilrs::Error::NotImplemented(gil)) => {
                    tracing::warn!("controllers are not supported on this platform");
                    gil
                }
                Err(e) => {
                    tracing::warn!("controller support unavailable: {e}");
                    return;
                }
            };

            for (gid, gamepad) in gil.gamepads() {
                joysticks.add_physical(gid, gamepad.name().to_string());
                on_event(InputEvent::Connected(gamepad.name().to_string()));
            }

            loop {
                let mut next = gil.next_event_blocking(Some(Duration::from_millis(10)));
                while let Some(event) = next {
                    match event.event {
                        EventType::Connected => {
                            let name = gil.gamepad(event.id).name().to_string();
                            joysticks.add_physical(event.id, name.clone());
                            on_event(InputEvent::Connected(name));
                        }
                        EventType::Disconnected => {
                            // The joystick lock is released before calling back into the controller
                            if let Some(removed) = joysticks.remove_physical(event.id) {
                                on_event(InputEvent::Removed(removed));
                            }
                        }
                        _ => {}
                    }
                    next = gil.next_event();
                }
                for (gid, gamepad) in gil.gamepads() {
                    joysticks.set_values(gid, mapping::gamepad_values(&gamepad));
                }
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("failed to start controller thread: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(js: &Joysticks) -> Vec<Option<Uuid>> {
        js.lock().slots.to_vec()
    }

    fn physical(js: &Joysticks, n: usize) -> Uuid {
        // GamepadId can't be constructed outside gilrs, so fake devices go in directly
        let id = Uuid::new_v4();
        let mut inner = js.lock();
        inner.devices.push(Device {
            id,
            name: format!("pad {n}"),
            kind: Kind::Virtual,
            values: vec![JoystickValue::Axis { id: 0, value: 0.5 }],
        });
        let slot = inner.first_free().unwrap();
        inner.slots[slot] = Some(id);
        id
    }

    #[test]
    fn empty_until_bound() {
        let js = Joysticks::default();
        assert!(js.supply().is_empty());
        let (slots, unbound) = js.slots();
        assert_eq!(slots.len(), SLOTS);
        assert_eq!(unbound[0].name, VIRTUAL_NAME);
    }

    #[test]
    fn virtual_joystick_reports_full_layout() {
        let js = Joysticks::default();
        assert_eq!(js.bind(VIRTUAL_ID, Some(2)), Ok(2));
        js.update_virtual(|v| {
            v.axes[1] = -1.0;
            v.buttons[0] = true;
        });
        let supplied = js.supply();
        assert_eq!(supplied.len(), 3);
        assert!(supplied[0].is_empty() && supplied[1].is_empty());
        assert_eq!(supplied[2].len(), 17);
        assert!(supplied[2].contains(&JoystickValue::Axis { id: 1, value: -1.0 }));
        assert!(supplied[2].contains(&JoystickValue::Button {
            id: 1,
            pressed: true
        }));
        js.reset_virtual();
        assert!(js.supply()[2].contains(&JoystickValue::POV { id: 0, angle: -1 }));
    }

    #[test]
    fn bind_swaps_and_move_shifts() {
        let js = Joysticks::default();
        let a = physical(&js, 0);
        let b = physical(&js, 1);
        assert_eq!(ids(&js)[..2], [Some(a), Some(b)]);

        // Binding into an occupied slot swaps
        js.bind(b, Some(0)).unwrap();
        assert_eq!(ids(&js)[..2], [Some(b), Some(a)]);

        // Binding an unbound device into an occupied slot unbinds the occupant
        js.bind(VIRTUAL_ID, Some(0)).unwrap();
        assert_eq!(ids(&js)[..2], [Some(VIRTUAL_ID), Some(a)]);
        assert_eq!(js.slots().1[0].id, b);

        js.move_slot(0, 3).unwrap();
        assert_eq!(ids(&js)[..4], [Some(a), None, None, Some(VIRTUAL_ID)]);
        js.move_slot(3, 0).unwrap();
        assert_eq!(ids(&js)[..2], [Some(VIRTUAL_ID), Some(a)]);

        js.unbind(a).unwrap();
        assert!(js.bind(Uuid::new_v4(), None).is_err());
        assert!(js.move_slot(0, 6).is_err());
    }
}
