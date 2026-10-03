use super::*;

#[test]
fn requires_srgb_swapchain() {
    let srgb = vk::Format::R8G8B8A8_SRGB;
    let linear = vk::Format::R8G8B8A8_UNORM;
    assert_eq!(
        panel_swapchain_format(&[linear.as_raw() as u32, srgb.as_raw() as u32])
            .unwrap()
            .as_raw(),
        srgb.as_raw()
    );
    assert!(panel_swapchain_format(&[linear.as_raw() as u32]).is_err());
    assert!(panel_swapchain_format(&[]).is_err());
}
