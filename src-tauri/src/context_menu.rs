//! Trims WebKit's own right-click menu.
//!
//! A message is shown in a sandboxed frame without scripts, and WebKit runs no event listeners
//! there, not even ones the app adds from outside. So the app can't block the native menu inside
//! a message from JavaScript, and WebKit offers "Open Image in New Window", "Download Image" and
//! "Open Frame in New Window", which make no sense here (and could load a blocked remote image).
//! This removes those items just before the menu opens; Copy and Look Up on a selection stay.
//! A menu left with nothing in it isn't shown.

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
use objc2::{msg_send, sel};
use objc2_foundation::NSString;
use std::ffi::c_void;
use std::sync::OnceLock;

/// WKWebView, whose own `willOpenMenu:withEvent:` still runs after ours.
static SUPER: OnceLock<&'static AnyClass> = OnceLock::new();

fn unwanted(id: &str) -> bool {
    id.contains("NewWindow") || id.starts_with("WKMenuItemIdentifierDownload")
}

unsafe fn item(menu: &AnyObject, i: isize) -> Option<Retained<AnyObject>> {
    msg_send![menu, itemAtIndex: i]
}

unsafe fn is_separator(item: &AnyObject) -> bool {
    let b: Bool = msg_send![item, isSeparatorItem];
    b.as_bool()
}

unsafe fn trim(menu: &AnyObject) {
    let count = |m: &AnyObject| -> isize { msg_send![m, numberOfItems] };
    for i in (0..count(menu)).rev() {
        let Some(it) = item(menu, i) else { continue };
        let id: Option<Retained<NSString>> = msg_send![&*it, identifier];
        if id.is_some_and(|id| unwanted(&id.to_string())) {
            let _: () = msg_send![menu, removeItemAtIndex: i];
        }
    }
    // Drop separators left at either end or next to each other.
    let mut i = 0;
    while i < count(menu) {
        let sep = item(menu, i).is_some_and(|it| is_separator(&it));
        let prev_sep = i == 0 || item(menu, i - 1).is_some_and(|it| is_separator(&it));
        if sep && (prev_sep || i == count(menu) - 1) {
            let _: () = msg_send![menu, removeItemAtIndex: i];
            // The one before may now be a trailing separator.
            i = (i - 1).max(0);
        } else {
            i += 1;
        }
    }
}

type WillOpenMenuFn = unsafe extern "C-unwind" fn(&AnyObject, Sel, *mut AnyObject, *mut AnyObject);

unsafe extern "C-unwind" fn will_open_menu(this: &AnyObject, _cmd: Sel, menu: *mut AnyObject, event: *mut AnyObject) {
    if !menu.is_null() {
        trim(&*menu);
    }
    if let Some(sup) = SUPER.get() {
        let _: () = msg_send![super(this, sup), willOpenMenu: menu, withEvent: event];
    }
}

/// Adds the override to the webview's class (wry's WKWebView subclass), once for all windows.
pub fn install(webview: *mut c_void) {
    if webview.is_null() || SUPER.get().is_some() {
        return;
    }
    unsafe {
        let obj = &*(webview as *const AnyObject);
        let cls = obj.class();
        let Some(sup) = cls.superclass() else { return };
        if SUPER.set(sup).is_err() {
            return;
        }
        let imp: WillOpenMenuFn = will_open_menu;
        let added = objc2::ffi::class_addMethod(
            cls as *const AnyClass as *mut AnyClass,
            sel!(willOpenMenu:withEvent:),
            std::mem::transmute::<WillOpenMenuFn, Imp>(imp),
            c"v@:@@".as_ptr(),
        );
        if !added.as_bool() {
            eprintln!("context_menu: willOpenMenu:withEvent: already defined on webview class");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2::runtime::{ClassBuilder, NSObject};
    use objc2::ClassType;
    use std::sync::atomic::{AtomicUsize, Ordering};

    unsafe fn menu_with(items: &[(&str, &str)]) -> Retained<AnyObject> {
        let menu: Retained<AnyObject> = msg_send![AnyClass::get(c"NSMenu").unwrap(), new];
        for (id, title) in items {
            let item: Retained<AnyObject> = if *id == "-" {
                msg_send![AnyClass::get(c"NSMenuItem").unwrap(), separatorItem]
            } else {
                let it: Retained<AnyObject> = msg_send![AnyClass::get(c"NSMenuItem").unwrap(), new];
                let _: () = msg_send![&*it, setTitle: &*NSString::from_str(title)];
                let _: () = msg_send![&*it, setIdentifier: &*NSString::from_str(id)];
                it
            };
            let _: () = msg_send![&*menu, addItem: &*item];
        }
        menu
    }

    unsafe fn titles(menu: &AnyObject) -> Vec<String> {
        let n: isize = msg_send![menu, numberOfItems];
        (0..n)
            .map(|i| {
                let it = item(menu, i).unwrap();
                if is_separator(&it) {
                    return "-".into();
                }
                let t: Retained<NSString> = msg_send![&*it, title];
                t.to_string()
            })
            .collect()
    }

    #[test]
    fn hook_trims_webkit_menus_and_calls_super() {
        unsafe {
            // Stand-ins for WKWebView (which records that its own method still runs) and wry's subclass.
            static SUPER_RAN: AtomicUsize = AtomicUsize::new(0);
            unsafe extern "C-unwind" fn base(_: &AnyObject, _: Sel, _: &AnyObject, _: *mut AnyObject) {
                SUPER_RAN.fetch_add(1, Ordering::SeqCst);
            }
            let mut b = ClassBuilder::new(c"MailvernTestWKWebView", NSObject::class()).unwrap();
            b.add_method(sel!(willOpenMenu:withEvent:), base as unsafe extern "C-unwind" fn(_, _, _, _));
            let base_cls = b.register();
            let cls = ClassBuilder::new(c"MailvernTestWryWebView", base_cls).unwrap().register();
            let view: Retained<AnyObject> = msg_send![cls, new];
            install(Retained::as_ptr(&view) as *mut c_void);

            // What WebKit showed on a blocked image and on empty space in a message.
            let image = menu_with(&[
                ("WKMenuItemIdentifierOpenImageInNewWindow", "Open Image in New Window"),
                ("WKMenuItemIdentifierDownloadImage", "Download Image"),
                ("-", ""),
            ]);
            let frame = menu_with(&[("WKMenuItemIdentifierOpenFrameInNewWindow", "Open Frame in New Window"), ("-", "")]);
            let selection = menu_with(&[
                ("WKMenuItemIdentifierLookUp", "Look Up"),
                ("-", ""),
                ("WKMenuItemIdentifierCopy", "Copy"),
                ("-", ""),
                ("WKMenuItemIdentifierOpenLinkInNewWindow", "Open Link in New Window"),
                ("WKMenuItemIdentifierDownloadLinkedFile", "Download Linked File"),
                ("-", ""),
                ("WKMenuItemIdentifierShareMenu", "Share"),
            ]);
            for m in [&image, &frame, &selection] {
                let _: () = msg_send![&*view, willOpenMenu: &**m, withEvent: std::ptr::null_mut::<AnyObject>()];
            }
            assert!(titles(&image).is_empty());
            assert!(titles(&frame).is_empty());
            assert_eq!(titles(&selection), ["Look Up", "-", "Copy", "-", "Share"]);
            assert_eq!(SUPER_RAN.load(Ordering::SeqCst), 3);
        }
    }
}
