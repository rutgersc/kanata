use block::ConcreteBlock;
use log::info;
use objc::runtime::{Class, Object};
use objc::{msg_send, sel, sel_impl};
use parking_lot::Mutex;
use std::ffi::CStr;
use std::sync::mpsc::SyncSender;
use std::sync::OnceLock;

use crate::oskbd::{KeyEvent, KeyValue};
use kanata_parser::keys::OsCode;

unsafe extern "C" {
    fn CFRunLoopRun();
}

static FOCUSED_APP: OnceLock<Mutex<String>> = OnceLock::new();
static WAKEUP_TX: OnceLock<SyncSender<KeyEvent>> = OnceLock::new();

unsafe fn nsstring_to_string(nsstring: *mut Object) -> Option<String> {
    if nsstring.is_null() {
        return None;
    }
    let utf8: *const i8 = msg_send![nsstring, UTF8String];
    if utf8.is_null() {
        return None;
    }
    Some(CStr::from_ptr(utf8).to_string_lossy().into_owned())
}

fn on_app_activated() {
    unsafe {
        let workspace: *mut Object =
            msg_send![Class::get("NSWorkspace").unwrap(), sharedWorkspace];
        let frontmost: *mut Object = msg_send![workspace, frontmostApplication];
        if frontmost.is_null() {
            return;
        }
        let name: *mut Object = msg_send![frontmost, localizedName];
        if let Some(app_name) = nsstring_to_string(name) {
            if let Some(focused) = FOCUSED_APP.get() {
                *focused.lock() = app_name;
            }
            if let Some(tx) = WAKEUP_TX.get() {
                let _ = tx.try_send(KeyEvent {
                    code: OsCode::KEY_RESERVED,
                    value: KeyValue::WakeUp,
                });
            }
        }
    }
}

/// Called from main.rs on the main thread. Sets up the NSWorkspace notification
/// observer, then runs CFRunLoop to pump macOS events forever. This function
/// never returns.
pub(super) fn start_app_focus_listener_on_main(wakeup_tx: SyncSender<KeyEvent>) {
    if WAKEUP_TX.set(wakeup_tx).is_err() {
        log::debug!("app focus listener already running, skipping");
        return;
    }
    FOCUSED_APP.get_or_init(|| Mutex::new(String::new()));
    info!("setting up app focus listener on main thread");

    unsafe {
        // Initialize AppKit — required for NSWorkspace notifications.
        if let Some(ns_app_class) = Class::get("NSApplication") {
            let _: *mut Object = msg_send![ns_app_class, sharedApplication];
        }

        let Some(ws_class) = Class::get("NSWorkspace") else {
            log::error!("NSWorkspace class not found");
            return;
        };
        let workspace: *mut Object = msg_send![ws_class, sharedWorkspace];
        let nc: *mut Object = msg_send![workspace, notificationCenter];

        let Some(nsstring_class) = Class::get("NSString") else {
            log::error!("NSString class not found");
            return;
        };
        let notification_name: *mut Object = msg_send![
            nsstring_class,
            stringWithUTF8String: "NSWorkspaceDidActivateApplicationNotification\0".as_ptr()
        ];

        let block = ConcreteBlock::new(|_notification: *mut Object| {
            on_app_activated();
        });
        let block = block.copy();

        // nil queue = deliver on posting thread (main thread), which is fine
        // since we're running CFRunLoop on main.
        let _: () = msg_send![
            nc,
            addObserverForName: notification_name
            object: std::ptr::null::<Object>()
            queue: std::ptr::null::<Object>()
            usingBlock: &*block
        ];

        std::mem::forget(block);

        info!("app focus listener started, running main CFRunLoop");
        CFRunLoopRun();
    }
}

pub(super) fn get_focused_app() -> Option<String> {
    let focused = FOCUSED_APP.get()?;
    let current = focused.lock();
    if current.is_empty() {
        None
    } else {
        Some(current.clone())
    }
}
