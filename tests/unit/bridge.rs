use super::*;

#[test]
fn reliable_input_bypasses_motion_capacity_and_preserves_order() {
    let (sender, receiver) = new_input_channel();
    for time_ms in 0..16 {
        sender
            .try_send(XrInput::Ray {
                ray: Ray3 {
                    origin: glam::Vec3::ZERO,
                    direction: glam::Vec3::NEG_Z,
                },
                gaze_ray: None,
                time_ms,
            })
            .unwrap();
    }
    assert!(matches!(
        sender.try_send(XrInput::PointerLost { time_ms: 16 }),
        Err(std::sync::mpsc::TrySendError::Full(_))
    ));
    for time_ms in 16..256 {
        sender
            .send(XrInput::Button {
                button: 0x110,
                pressed: time_ms % 2 == 0,
                time_ms,
            })
            .unwrap();
    }
    for expected in 0..256 {
        let event = receiver.try_recv().unwrap();
        sender.received(&event);
        match event {
            XrInput::Ray { time_ms, .. } => assert_eq!(time_ms, expected),
            XrInput::Button {
                time_ms, pressed, ..
            } => {
                assert_eq!(time_ms, expected);
                assert_eq!(pressed, expected % 2 == 0);
            }
            _ => panic!("unexpected event"),
        }
    }
    sender
        .try_send(XrInput::PointerLost { time_ms: 256 })
        .unwrap();
}

#[test]
fn frame_requests_coalesce_until_consumed() {
    let (sender, receiver) = new_input_channel();
    for _ in 0..1000 {
        sender.request_frame().unwrap();
    }
    let event = receiver.try_recv().unwrap();
    assert!(matches!(event, XrInput::FrameTick));
    assert!(receiver.try_recv().is_err());
    sender.received(&event);
    sender.request_frame().unwrap();
    assert!(matches!(receiver.try_recv().unwrap(), XrInput::FrameTick));
}

#[test]
fn removals_are_not_lost_when_many_windows_update() {
    let (sender, receiver) = new_panel_channel();
    for panel_id in 0..32 {
        sender.publish(PanelUpdate::Removed {
            panel_id: crate::panel::PanelId::new(panel_id),
        });
        sender.publish(PanelUpdate::Removed {
            panel_id: crate::panel::PanelId::new(panel_id),
        });
    }
    assert_eq!(receiver.drain().len(), 32);
    assert!(receiver.drain().is_empty());
}
