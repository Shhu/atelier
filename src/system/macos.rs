//! macOS: Atelier's own app menu (winit's default one quits at once with
//! Cmd+Q, unsaved documents lost), and the application delegate taught to
//! open the files the Finder sends (Open, Open With, a drop on the Dock
//! icon) and to ask before quitting (Quit, the Dock's Quit, a logout).
//!
//! winit owns the delegate and checks its class, so it isn't replaced: its
//! class gets the two methods it lacks, when the app is about to finish
//! launching (before the Finder's files arrive).

use std::ffi::CStr;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{ClassType, DeclaredClass, declare_class, msg_send_id, mutability, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationTerminateReply, NSApplicationWillFinishLaunchingNotification,
    NSEventModifierFlags, NSMenu, NSMenuItem,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSNotification, NSNotificationCenter, NSObject, NSString, NSURL,
};

use crate::i18n::t;

/// Before the event loop is built: no default menu, and the delegate
/// completed once the app is about to finish launching.
pub fn configure(builder: &mut winit::event_loop::EventLoopBuilder<eframe::UserEvent>) {
    use winit::platform::macos::EventLoopBuilderExtMacOS as _;
    builder.with_default_menu(false);
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let observer = Launch::new(mtm);
    // SAFETY: the observer lives for the whole run (leaked below), and the
    // selector takes the notification as its argument.
    unsafe {
        NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
            &observer,
            sel!(willFinishLaunching:),
            Some(NSApplicationWillFinishLaunchingNotification),
            None,
        );
    }
    std::mem::forget(observer);
}

declare_class!(
    struct Launch;

    unsafe impl ClassType for Launch {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "AtelierLaunch";
    }

    impl DeclaredClass for Launch {}

    unsafe impl Launch {
        #[method(willFinishLaunching:)]
        fn will_finish_launching(&self, _notification: &NSNotification) {
            let mtm = MainThreadMarker::from(self);
            let app = NSApplication::sharedApplication(mtm);
            complete_delegate(&app);
            app.setMainMenu(Some(&app_menu(mtm)));
        }
    }
);

impl Launch {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        // SAFETY: NSObject's plain init.
        unsafe { msg_send_id![super(mtm.alloc().set_ivars(())), init] }
    }
}

/// Adds `applicationShouldTerminate:` and `application:openURLs:` to the
/// class of winit's delegate, then sets the delegate again: AppKit looks
/// at which methods a delegate has when it is set.
fn complete_delegate(app: &NSApplication) {
    // SAFETY: the delegate is winit's object; methods are only added to its
    // class (winit has neither), with matching signatures and encodings.
    unsafe {
        let Some(delegate) = app.delegate() else {
            return;
        };
        let object = Retained::as_ptr(&delegate) as *mut objc2::ffi::objc_object;
        let class = objc2::ffi::object_getClass(object) as *mut objc2::ffi::objc_class;
        let terminate: extern "C" fn(&AnyObject, Sel, &AnyObject) -> NSApplicationTerminateReply =
            should_terminate;
        let open: extern "C" fn(&AnyObject, Sel, &AnyObject, &NSArray<NSURL>) = open_urls;
        add_method(class, sel!(applicationShouldTerminate:), terminate as *const (), c"Q@:@");
        add_method(class, sel!(application:openURLs:), open as *const (), c"v@:@@");
        app.setDelegate(None);
        app.setDelegate(Some(&delegate));
    }
}

/// # Safety
/// `imp` is an `extern "C"` function of the method's signature, `types`
/// its encoding.
unsafe fn add_method(class: *mut objc2::ffi::objc_class, sel: Sel, imp: *const (), types: &CStr) {
    // SAFETY: as the caller promises.
    unsafe {
        let imp: objc2::ffi::IMP = Some(std::mem::transmute::<*const (), unsafe extern "C" fn()>(imp));
        objc2::ffi::class_addMethod(class, sel.as_ptr().cast(), imp, types.as_ptr());
    }
}

/// Quit, from the menu, the Dock or a logout: asked like the window's close
/// button (each unsaved document first), which then ends the app itself.
/// A logout is cancelled meanwhile; the user logs out again once saved.
extern "C" fn should_terminate(_: &AnyObject, _: Sel, _: &AnyObject) -> NSApplicationTerminateReply {
    super::quit();
    NSApplicationTerminateReply::NSTerminateCancel
}

/// Files from the Finder or the Dock, opened as tabs.
extern "C" fn open_urls(_: &AnyObject, _: Sel, _: &AnyObject, urls: &NSArray<NSURL>) {
    // SAFETY: plain NSURL getters.
    let paths = urls.iter().filter_map(|url| unsafe {
        url.isFileURL().then(|| url.path()).flatten().map(|p| p.to_string().into())
    });
    super::open(paths.collect::<Vec<_>>());
}

/// The app menu: About, Hide, Hide Others, Show All, Quit (Cmd+Q, through
/// `terminate:` like the Dock's Quit).
fn app_menu(mtm: MainThreadMarker) -> Retained<NSMenu> {
    let item = |title: String, action: Option<Sel>, key: &str| {
        // SAFETY: standard NSApplication actions, sent along the responder
        // chain.
        unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                &NSString::from_str(&title),
                action,
                &NSString::from_str(key),
            )
        }
    };
    let menu = NSMenu::new(mtm);
    menu.addItem(&item(
        t!("platforms14-mac-about"),
        Some(sel!(orderFrontStandardAboutPanel:)),
        "",
    ));
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&item(t!("platforms14-mac-hide"), Some(sel!(hide:)), "h"));
    let others = item(
        t!("platforms14-mac-hide-others"),
        Some(sel!(hideOtherApplications:)),
        "h",
    );
    others.setKeyEquivalentModifierMask(
        NSEventModifierFlags::NSEventModifierFlagOption
            | NSEventModifierFlags::NSEventModifierFlagCommand,
    );
    menu.addItem(&others);
    menu.addItem(&item(
        t!("platforms14-mac-show-all"),
        Some(sel!(unhideAllApplications:)),
        "",
    ));
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&item(t!("platforms14-mac-quit"), Some(sel!(terminate:)), "q"));
    let app_item = NSMenuItem::new(mtm);
    app_item.setSubmenu(Some(&menu));
    let bar = NSMenu::new(mtm);
    bar.addItem(&app_item);
    bar
}
