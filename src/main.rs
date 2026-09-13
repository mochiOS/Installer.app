use viewkit::prelude::*;

struct InstallerApp;

impl App for InstallerApp {
	type Body = Box<dyn View + 'static>;
	
	fn new() -> Self {
		Self
	}
	
	fn window(&self) -> WindowOptions {
		WindowOptions::new("Installer")
			.size(800.0, 560.0)
			.resizable(true)
	}
	
	fn body(&self, _context: &ViewContext) -> Self::Body {
		Box::new(VStack::new())
	}
}

fn main() -> Result<(), ViewKitError> {
	viewkit::run::<InstallerApp>()
}
