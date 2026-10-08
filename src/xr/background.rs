use super::*;
use std::thread::JoinHandle;

pub(super) struct PendingSkybox {
    image: String,
    pub worker: JoinHandle<Result<SkyboxTexture>>,
}

impl PendingSkybox {
    fn new(
        scene: &SceneRenderer,
        instance: &ash::Instance,
        physical_device: vk::PhysicalDevice,
        image: String,
    ) -> Result<Self> {
        let worker = SkyboxTexture::load_async(scene, instance, physical_device, &image)?;
        Ok(Self { image, worker })
    }
}

pub(super) struct Background {
    pub skybox: Option<SkyboxTexture>,
    pub pending: Option<PendingSkybox>,
    pub exposure: f32,
    exposure_target: f32,
    queued_image: Option<String>,
}

impl Background {
    pub(super) fn new(
        scene: &mut SceneRenderer,
        instance: &ash::Instance,
        physical_device: vk::PhysicalDevice,
        config: &AppConfig,
    ) -> Result<Self> {
        let pending = Some(PendingSkybox::new(
            scene,
            instance,
            physical_device,
            config.background.image.clone(),
        )?);
        scene.set_background_exposure(0.0, config)?;
        Ok(Self {
            skybox: None,
            pending,
            exposure: 0.0,
            exposure_target: 0.0,
            queued_image: None,
        })
    }

    pub(super) fn poll(
        &mut self,
        scene: &mut SceneRenderer,
        instance: &ash::Instance,
        physical_device: vk::PhysicalDevice,
        config: &AppConfig,
    ) -> Result<()> {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.worker.is_finished())
        {
            let pending = self.pending.take().expect("finished skybox load exists");
            match pending.worker.join() {
                Ok(Ok(loaded)) if pending.image == config.background.image => {
                    scene.update_skybox_diffuse(loaded.diffuse_irradiance(), config)?;
                    self.skybox = Some(loaded);
                    self.exposure_target = 1.0;
                }
                Ok(Ok(_)) => {}
                Ok(Err(error)) => {
                    eprintln!("skybox load failed: {error:#}");
                    self.exposure_target = 1.0;
                }
                Err(_) => {
                    eprintln!("skybox loading thread panicked");
                    self.exposure_target = 1.0;
                }
            }
            if let Some(image) = self.queued_image.take() {
                self.pending = Some(PendingSkybox::new(scene, instance, physical_device, image)?);
                self.exposure_target = 0.0;
            }
        }
        Ok(())
    }

    pub(super) fn reload(
        &mut self,
        image: String,
        scene: &SceneRenderer,
        instance: &ash::Instance,
        physical_device: vk::PhysicalDevice,
    ) {
        if self.pending.is_some() {
            self.queued_image = Some(image);
            self.exposure_target = 0.0;
        } else {
            match PendingSkybox::new(scene, instance, physical_device, image) {
                Ok(pending) => {
                    self.pending = Some(pending);
                    self.exposure_target = 0.0;
                }
                Err(error) => {
                    eprintln!("skybox load could not start: {error:#}");
                    self.exposure_target = 1.0;
                }
            }
        }
    }

    pub(super) fn animate(&mut self, smoothing: f32) {
        self.exposure += (self.exposure_target - self.exposure) * smoothing;
        if (self.exposure_target - self.exposure).abs() < 0.0001 {
            self.exposure = self.exposure_target;
        }
    }
}
