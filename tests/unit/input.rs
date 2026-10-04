use super::*;

#[test]
fn overlapping_devices_keep_keys_held_until_last_release() {
    let mut counts = BTreeMap::new();
    assert!(!transition(&mut counts, 42, false));
    assert!(transition(&mut counts, 42, true));
    assert!(!transition(&mut counts, 42, true));
    assert!(!transition(&mut counts, 42, false));
    assert!(transition(&mut counts, 42, false));
    assert!(counts.is_empty());
}

#[test]
fn automatic_selection_defaults_to_seat0_and_respects_session_seat() {
    assert_eq!(
        Selection::new(None, None),
        Selection::Automatic {
            seat: "seat0".into()
        }
    );
    assert_eq!(
        Selection::new(None, Some("".into())),
        Selection::Automatic {
            seat: "seat0".into()
        }
    );
    assert_eq!(
        Selection::new(None, Some("seat1".into())),
        Selection::Automatic {
            seat: "seat1".into()
        }
    );
}

#[test]
fn explicit_paths_override_discovery_and_empty_override_disables_input() {
    let selection = Selection::new(
        Some("/dev/input/event1:/dev/input/by-id/keyboard".into()),
        Some("seat1".into()),
    );
    assert_eq!(
        selection,
        Selection::Explicit(vec![
            "/dev/input/event1".into(),
            "/dev/input/by-id/keyboard".into(),
        ])
    );
    assert!(
        Selection::new(Some("".into()), None)
            .paths()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn discovery_only_accepts_keyboards_and_mice_on_the_current_seat() {
    let seat0 = OsStr::new("seat0");
    let seat1 = OsStr::new("seat1");
    let yes = Some(OsStr::new("1"));
    assert!(eligible_device(seat0, None, yes, None));
    assert!(eligible_device(seat0, Some(seat0), None, yes));
    assert!(eligible_device(seat1, Some(seat1), yes, yes));
    assert!(!eligible_device(seat1, None, yes, yes));
    assert!(!eligible_device(seat0, Some(seat1), yes, yes));
    assert!(!eligible_device(seat0, None, None, None));
    assert!(!eligible_device(seat0, None, Some(OsStr::new("0")), None));
}

#[test]
fn aliases_share_identity_and_replacement_changes_identity() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("event0");
    let alias = directory.path().join("keyboard");
    fs::write(&path, []).unwrap();
    std::os::unix::fs::symlink(&path, &alias).unwrap();
    let original = DeviceIdentity::at(&path).unwrap();
    assert_eq!(original, DeviceIdentity::at(&alias).unwrap());
    let replacement = directory.path().join("replacement");
    fs::write(&replacement, []).unwrap();
    fs::rename(replacement, &path).unwrap();
    assert_ne!(original, DeviceIdentity::at(&path).unwrap());
    assert_eq!(
        DeviceIdentity::at(&path).unwrap(),
        DeviceIdentity::at(&alias).unwrap()
    );
}

#[test]
fn releasing_a_disconnected_device_is_idempotent() {
    let mut state = DeviceState {
        pressed: BTreeSet::from([30, 42, 0x110]),
        disconnected: true,
    };
    let mut released = Vec::new();
    state.release_all(|code| released.push(code));
    state.release_all(|code| released.push(code));
    assert_eq!(released, [30, 42, 0x110]);
    assert!(state.pressed.is_empty());
}

fn track_test_device(handle: &LoopHandle<'static, Compositor>, path: &Path) -> TrackedDevice {
    TrackedDevice {
        identity: DeviceIdentity::at(path).unwrap(),
        token: handle
            .insert_source(Timer::from_duration(Duration::from_secs(60)), |_, _, _| {
                TimeoutAction::Drop
            })
            .unwrap(),
        state: Rc::new(RefCell::new(DeviceState::default())),
    }
}

#[test]
fn duplicate_device_aliases_are_not_opened_twice() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("event0");
    let alias = directory.path().join("keyboard");
    fs::write(&path, []).unwrap();
    std::os::unix::fs::symlink(&path, &alias).unwrap();
    let event_loop = smithay::reexports::calloop::EventLoop::<Compositor>::try_new().unwrap();
    let handle = event_loop.handle();
    let device = track_test_device(&handle, &path);
    let mut manager = DeviceManager {
        selection: Selection::Explicit(vec![path.clone(), alias.clone()]),
        devices: BTreeMap::from([(path, device)]),
        failures: BTreeMap::new(),
    };
    let paths = manager.selection.paths().unwrap();
    let identities = manager.identities(&paths);
    // Opening the regular file would fail if alias deduplication did not work.
    manager.connect(&handle, identities, true).unwrap();
    assert_eq!(manager.devices.len(), 1);
    assert!(manager.failures.is_empty());
}

#[test]
fn hotplug_removal_releases_only_the_removed_devices_keys_and_buttons() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("event0");
    fs::write(&path, []).unwrap();
    let event_loop = smithay::reexports::calloop::EventLoop::<Compositor>::try_new().unwrap();
    let handle = event_loop.handle();
    let device = track_test_device(&handle, &path);
    device.state.borrow_mut().pressed.extend([30, 0x110]);
    let state = device.state.clone();
    let mut manager = DeviceManager {
        selection: Selection::Explicit(vec![path.clone()]),
        devices: BTreeMap::from([(path.clone(), device)]),
        failures: BTreeMap::new(),
    };
    let display = smithay::reexports::wayland_server::Display::<Compositor>::new().unwrap();
    let (sender, _receiver) = crate::bridge::panel_channel();
    let mut compositor = Compositor::new(display.handle(), sender);
    compositor.dispatch_key(30, true, 0);
    compositor.dispatch_key(30, true, 0);
    compositor.dispatch_pointer_button(0x110, true, 0);

    fs::remove_file(&path).unwrap();
    manager.refresh(&handle, &mut compositor).unwrap();
    assert!(manager.devices.is_empty());
    assert!(state.borrow().pressed.is_empty());
    assert_eq!(compositor.key_counts.get(&30), Some(&1));
    assert!(compositor.button_counts.is_empty());
    assert!(manager.failures.contains_key(&path));

    manager.refresh(&handle, &mut compositor).unwrap();
    assert_eq!(compositor.key_counts.get(&30), Some(&1));
}

#[test]
fn replacement_and_read_failure_both_require_reconnection() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("event0");
    fs::write(&path, []).unwrap();
    let event_loop = smithay::reexports::calloop::EventLoop::<Compositor>::try_new().unwrap();
    let handle = event_loop.handle();
    let device = track_test_device(&handle, &path);
    let identity = device.identity;
    assert!(!device.needs_removal(Some(&identity)));
    assert!(device.needs_removal(None));

    let replacement = directory.path().join("replacement");
    fs::write(&replacement, []).unwrap();
    fs::rename(replacement, &path).unwrap();
    assert!(device.needs_removal(Some(&DeviceIdentity::at(&path).unwrap())));

    device.state.borrow_mut().disconnected = true;
    assert!(device.needs_removal(Some(&identity)));
}

#[test]
fn failed_connections_remain_retryable_and_explicit_startup_errors_are_fatal() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("event0");
    let event_loop = smithay::reexports::calloop::EventLoop::<Compositor>::try_new().unwrap();
    let handle = event_loop.handle();
    let mut manager = DeviceManager {
        selection: Selection::Explicit(vec![path.clone()]),
        devices: BTreeMap::new(),
        failures: BTreeMap::new(),
    };
    let paths = manager.selection.paths().unwrap();
    assert!(manager.identities(&paths).is_empty());
    assert!(manager.failures.contains_key(&path));

    fs::write(&path, []).unwrap();
    let identities = manager.identities(&paths);
    assert_eq!(identities.len(), 1);
    assert!(manager.connect(&handle, identities.clone(), true).is_err());
    manager.connect(&handle, identities, false).unwrap();
    assert!(manager.devices.is_empty());
    assert!(manager.failures[&path].contains("open input device"));
}
