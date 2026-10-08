use super::*;

pub(super) struct Actions {
    pub action_set: xr::ActionSet,
    pub right_hand: xr::Path,
    pub left_hand: xr::Path,
    pub aim_action: xr::Action<xr::Posef>,
    pub trigger_action: xr::Action<bool>,
    pub secondary_action: xr::Action<bool>,
    pub launcher_action: xr::Action<bool>,
    pub face_click_action: xr::Action<bool>,
    pub stick_click_action: xr::Action<bool>,
    pub grip_action: xr::Action<bool>,
    pub stick_action: xr::Action<xr::Vector2f>,
    pub aim_space: xr::Space,
}

impl Actions {
    pub(super) fn new(instance: &xr::Instance, session: &xr::Session<xr::Vulkan>) -> Result<Self> {
        let action_set = instance.create_action_set("spacetop", "Spacetop input", 0)?;
        let right_hand = instance.string_to_path("/user/hand/right")?;
        let left_hand = instance.string_to_path("/user/hand/left")?;
        let aim_action =
            action_set.create_action::<xr::Posef>("aim_pose", "Aim pose", &[right_hand])?;
        let trigger_action =
            action_set.create_action::<bool>("trigger", "Trigger", &[right_hand])?;
        let secondary_action =
            action_set.create_action::<bool>("secondary", "Secondary click", &[left_hand])?;
        let launcher_action =
            action_set.create_action::<bool>("launcher", "Launcher toggle", &[right_hand])?;
        let face_click_action = action_set.create_action::<bool>(
            "face_click",
            "Face button click",
            &[left_hand, right_hand],
        )?;
        let stick_click_action =
            action_set.create_action::<bool>("stick_click", "Thumbstick click", &[right_hand])?;
        let grip_action = action_set.create_action::<bool>("grip", "Grip", &[right_hand])?;
        let stick_action =
            action_set.create_action::<xr::Vector2f>("stick", "Thumbstick", &[right_hand])?;
        let aim_path = instance.string_to_path("/user/hand/right/input/aim/pose")?;
        for (
            profile,
            button,
            launcher,
            left_face,
            right_face,
            left_secondary,
            grip,
            stick,
            stick_click,
        ) in [
            (
                "/interaction_profiles/khr/simple_controller",
                "select/click",
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            ),
            (
                "/interaction_profiles/oculus/touch_controller",
                "trigger/value",
                Some("b/click"),
                Some("x/click"),
                Some("a/click"),
                Some("y/click"),
                Some("squeeze/value"),
                Some("thumbstick"),
                Some("thumbstick/click"),
            ),
            (
                "/interaction_profiles/valve/index_controller",
                "trigger/click",
                Some("b/click"),
                Some("a/click"),
                Some("a/click"),
                Some("b/click"),
                Some("squeeze/value"),
                Some("thumbstick"),
                Some("thumbstick/click"),
            ),
            (
                "/interaction_profiles/htc/vive_controller",
                "trigger/click",
                Some("trackpad/click"),
                None,
                None,
                Some("trackpad/click"),
                Some("squeeze/click"),
                Some("trackpad"),
                None,
            ),
            (
                "/interaction_profiles/microsoft/motion_controller",
                "trigger/value",
                Some("trackpad/click"),
                None,
                None,
                Some("trackpad/click"),
                Some("squeeze/click"),
                Some("thumbstick"),
                Some("thumbstick/click"),
            ),
        ] {
            let mut bindings = vec![
                xr::Binding::new(&aim_action, aim_path),
                xr::Binding::new(
                    &trigger_action,
                    instance.string_to_path(&format!("/user/hand/right/input/{button}"))?,
                ),
            ];
            if let Some(launcher) = launcher {
                bindings.push(xr::Binding::new(
                    &launcher_action,
                    instance.string_to_path(&format!("/user/hand/right/input/{launcher}"))?,
                ));
            }
            if let Some(left_face) = left_face {
                bindings.push(xr::Binding::new(
                    &face_click_action,
                    instance.string_to_path(&format!("/user/hand/left/input/{left_face}"))?,
                ));
            }
            if let Some(right_face) = right_face {
                bindings.push(xr::Binding::new(
                    &face_click_action,
                    instance.string_to_path(&format!("/user/hand/right/input/{right_face}"))?,
                ));
            }
            if let Some(secondary) = left_secondary {
                bindings.push(xr::Binding::new(
                    &secondary_action,
                    instance.string_to_path(&format!("/user/hand/left/input/{secondary}"))?,
                ));
            }
            if let Some(grip) = grip {
                bindings.push(xr::Binding::new(
                    &grip_action,
                    instance.string_to_path(&format!("/user/hand/right/input/{grip}"))?,
                ));
            }
            if let Some(stick) = stick {
                bindings.push(xr::Binding::new(
                    &stick_action,
                    instance.string_to_path(&format!("/user/hand/right/input/{stick}"))?,
                ));
            }
            if let Some(stick_click) = stick_click {
                bindings.push(xr::Binding::new(
                    &stick_click_action,
                    instance.string_to_path(&format!("/user/hand/right/input/{stick_click}"))?,
                ));
            }
            instance.suggest_interaction_profile_bindings(
                instance.string_to_path(profile)?,
                &bindings,
            )?;
        }
        session.attach_action_sets(&[&action_set])?;
        let aim_space = aim_action.create_space(session, right_hand, xr::Posef::IDENTITY)?;
        Ok(Self {
            action_set,
            right_hand,
            left_hand,
            aim_action,
            trigger_action,
            secondary_action,
            launcher_action,
            face_click_action,
            stick_click_action,
            grip_action,
            stick_action,
            aim_space,
        })
    }
}
