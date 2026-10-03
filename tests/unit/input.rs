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
