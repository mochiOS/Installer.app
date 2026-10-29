mod backend;
mod native;
mod controller;
mod documents;
mod engine;
mod image_manifest;
mod deployment;
mod target_data;
mod model;
#[cfg(test)]
mod tests;
mod ui;
use appcore::prelude::*;
use std::rc::Rc;
struct InstallerApp {
    core: Rc<controller::Controller>,
}
impl App for InstallerApp {
    type Body = ui::InstallerView;
    fn new() -> Self {
        Self {
            core: controller::Controller::new(),
        }
    }
    fn window(&self) -> WindowOptions {
        WindowOptions::new("Install mochiOS Kinako")
            .size(1040.0, 760.0)
            .resizable(true)
    }
    fn close_requested(&mut self) -> bool {
        self.core.model.borrow().busy != Some(model::Busy::Installing)
    }
    fn body(&self, _: &ViewContext) -> Self::Body {
        ui::InstallerView::new(self.core.clone())
    }
}
fn main() -> Result<(), ViewKitError> {
    appcore::run::<InstallerApp>()
}
