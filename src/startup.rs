#[cfg(windows)]
pub fn is_packaged() -> bool {
    let Some(_apartment) = Apartment::initialize() else {
        return false;
    };
    windows::ApplicationModel::Package::Current().is_ok()
}

#[cfg(not(windows))]
pub fn is_packaged() -> bool {
    false
}

#[cfg(windows)]
pub fn launched_by_startup_task() -> bool {
    use windows::ApplicationModel::{Activation::ActivationKind, AppInstance};

    let Some(_apartment) = Apartment::initialize() else {
        return false;
    };
    AppInstance::GetActivatedEventArgs()
        .and_then(|arguments| arguments.Kind())
        .is_ok_and(|kind| kind == ActivationKind::StartupTask)
}

#[cfg(windows)]
struct Apartment;

#[cfg(windows)]
impl Apartment {
    fn initialize() -> Option<Self> {
        use windows::Win32::System::WinRT::{RO_INIT_SINGLETHREADED, RoInitialize};

        // SAFETY: this runs on the GUI's main thread before eframe starts its
        // event loop, and the matching RoUninitialize is issued by Drop.
        unsafe { RoInitialize(RO_INIT_SINGLETHREADED).ok()? };
        Some(Self)
    }
}

#[cfg(windows)]
impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: Apartment exists only after a successful RoInitialize call
        // on this thread and is dropped on that same thread.
        unsafe { windows::Win32::System::WinRT::RoUninitialize() };
    }
}

#[cfg(not(windows))]
pub fn launched_by_startup_task() -> bool {
    false
}
