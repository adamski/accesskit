use accesskit::{
    ActionHandler, ActionRequest, ActivationHandler, DeactivationHandler, Size, TreeUpdate, Vec2,
};
use accesskit_ios::Adapter;
use example_common::UiState;
use objc2::{
    ClassType, DefinedClass, MainThreadOnly, define_class, msg_send,
    rc::{Allocated, Retained, Weak},
    runtime::{AnyObject, Bool, NSObjectProtocol, Sel},
    sel,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSCopying, NSDictionary, NSObject, NSPoint, NSRect,
    NSRunLoopCommonModes, NSString,
};
use objc2_ui_kit::{
    UIApplication, UIApplicationDelegate, UIApplicationLaunchOptionsKey, UIColor, UIResponder,
    UIScene, UISceneConnectionOptions, UISceneDelegate, UISceneSession, UIView, UIViewAutoresizing,
    UIViewController, UIWindow, UIWindowScene, UIWindowSceneDelegate,
};
use std::{cell::RefCell, ffi::c_void, ptr::null_mut};

// The handlers only hold weak references to the view, which owns the adapter,
// which owns the handlers.
struct ShowcaseActivationHandler {
    view: Weak<ContentView>,
}

impl ActivationHandler for ShowcaseActivationHandler {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        let view = self.view.load()?;
        view.refresh_viewport();
        Some(view.ivars().ui.borrow_mut().build_tree_update())
    }
}

struct ShowcaseActionHandler {
    view: Weak<ContentView>,
}

impl ActionHandler for ShowcaseActionHandler {
    fn do_action(&mut self, request: ActionRequest) {
        let Some(view) = self.view.load() else {
            return;
        };
        // The adapter is handling a UIKit call, so the tree can't be updated
        // until the run loop comes back around.
        let mut pending_actions = view.ivars().pending_actions.borrow_mut();
        let schedule = pending_actions.is_empty();
        pending_actions.push(request);
        drop(pending_actions);
        if schedule {
            view.perform_after_delay(sel!(processPendingActions), 0.0);
        }
    }
}

struct ShowcaseDeactivationHandler {
    view: Weak<ContentView>,
}

impl DeactivationHandler for ShowcaseDeactivationHandler {
    fn deactivate_accessibility(&mut self) {
        if let Some(view) = self.view.load() {
            view.ivars().ui.borrow_mut().deactivated();
        }
    }
}

struct ContentViewIvars {
    // Created once the view it points to is initialized.
    adapter: RefCell<Option<Adapter>>,
    ui: RefCell<UiState>,
    pending_actions: RefCell<Vec<ActionRequest>>,
}

define_class!(
    #[unsafe(super(UIView, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AccessKitExampleContentView"]
    #[ivars = ContentViewIvars]
    struct ContentView;

    impl ContentView {
        #[unsafe(method(isAccessibilityElement))]
        fn is_accessibility_element(&self) -> Bool {
            match &*self.ivars().adapter.borrow() {
                Some(adapter) => Bool::new(adapter.is_accessibility_element()),
                None => Bool::NO,
            }
        }

        #[unsafe(method(accessibilityElements))]
        fn accessibility_elements(&self) -> *mut NSArray<NSObject> {
            match &*self.ivars().adapter.borrow() {
                Some(adapter) => adapter.accessibility_elements(),
                None => Retained::autorelease_return(NSArray::new()),
            }
        }

        #[unsafe(method(accessibilityHitTest:))]
        fn accessibility_hit_test(&self, point: NSPoint) -> *mut NSObject {
            match &*self.ivars().adapter.borrow() {
                Some(adapter) => adapter.hit_test(point),
                None => null_mut(),
            }
        }

        #[unsafe(method(didMoveToWindow))]
        fn did_move_to_window(&self) {
            unsafe {
                let _: () = msg_send![super(self), didMoveToWindow];
            }
            if self.window().is_none() {
                return;
            }
            if self.refresh_viewport() {
                self.update();
            }
            let events = self.ivars().adapter.borrow().as_ref().and_then(Adapter::view_did_appear);
            if let Some(events) = events {
                events.raise();
            }
        }

        #[unsafe(method(layoutSubviews))]
        fn layout_subviews(&self) {
            unsafe {
                let _: () = msg_send![super(self), layoutSubviews];
            }
            if self.refresh_viewport() {
                self.update();
            }
        }

        #[unsafe(method(processPendingActions))]
        fn process_pending_actions(&self) {
            let requests = self.ivars().pending_actions.take();
            {
                let mut ui = self.ivars().ui.borrow_mut();
                for request in &requests {
                    ui.do_action(request);
                }
            }
            self.after_input();
        }

        #[unsafe(method(processDueMessages))]
        fn process_due_messages(&self) {
            let processed = self.ivars().ui.borrow_mut().process_due_messages();
            if processed {
                self.update();
            }
            self.schedule_next_message();
        }
    }
);

impl ContentView {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(ContentViewIvars {
            adapter: RefCell::new(None),
            ui: RefCell::new(UiState::new()),
            pending_actions: RefCell::new(Vec::new()),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };

        this.setBackgroundColor(Some(&UIColor::darkGrayColor()));
        this.setAutoresizingMask(
            UIViewAutoresizing::FlexibleWidth | UIViewAutoresizing::FlexibleHeight,
        );

        let weak = Weak::from_retained(&this);
        // SAFETY: `this` is a valid, retained `UIView`, and it outlives the
        // adapter, which is owned by the view itself.
        let adapter = unsafe {
            Adapter::new(
                Retained::as_ptr(&this) as *mut c_void,
                ShowcaseActivationHandler { view: weak.clone() },
                ShowcaseActionHandler { view: weak.clone() },
                ShowcaseDeactivationHandler { view: weak },
            )
        };
        *this.ivars().adapter.borrow_mut() = Some(adapter);

        this
    }

    /// Returns whether the viewport changed.
    fn refresh_viewport(&self) -> bool {
        let bounds = self.bounds();
        let insets = self.safeAreaInsets();
        let scale_factor = self.contentScaleFactor();
        self.ivars().ui.borrow_mut().set_viewport(
            scale_factor,
            Vec2::new(insets.left * scale_factor, insets.top * scale_factor),
            Size::new(
                (bounds.size.width - insets.left - insets.right).max(0.0),
                (bounds.size.height - insets.top - insets.bottom).max(0.0),
            ),
        )
    }

    fn update(&self) {
        // Raising the events may call back into the view, which borrows the
        // adapter.
        let events = {
            let adapter = self.ivars().adapter.borrow();
            match &*adapter {
                Some(adapter) => {
                    adapter.update_if_active(|| self.ivars().ui.borrow_mut().build_tree_update())
                }
                None => None,
            }
        };
        if let Some(events) = events {
            events.raise();
        }
    }

    fn after_input(&self) {
        self.update();
        self.schedule_next_message();
    }

    fn schedule_next_message(&self) {
        unsafe {
            let _: () = msg_send![
                NSObject::class(),
                cancelPreviousPerformRequestsWithTarget: self,
                selector: sel!(processDueMessages),
                object: null_mut::<AnyObject>(),
            ];
        }
        let Some(delay) = self.ivars().ui.borrow().time_until_next_message() else {
            return;
        };
        self.perform_after_delay(sel!(processDueMessages), delay.as_secs_f64());
    }

    fn perform_after_delay(&self, selector: Sel, delay: f64) {
        let modes = NSArray::from_retained_slice(&[unsafe { NSRunLoopCommonModes }.copy()]);
        unsafe {
            let _: () = msg_send![
                self,
                performSelector: selector,
                withObject: null_mut::<AnyObject>(),
                afterDelay: delay,
                inModes: &*modes,
            ];
        }
    }
}

define_class!(
    #[unsafe(super(UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AccessKitExampleAppDelegate"]
    struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    // The scene delegate, which the Info.plist names, sets up the window.
    unsafe impl UIApplicationDelegate for AppDelegate {
        #[unsafe(method(application:didFinishLaunchingWithOptions:))]
        fn did_finish_launching(
            &self,
            _application: &UIApplication,
            _launch_options: Option<&NSDictionary<UIApplicationLaunchOptionsKey, AnyObject>>,
        ) -> bool {
            true
        }
    }
);

struct SceneDelegateIvars {
    window: RefCell<Option<Retained<UIWindow>>>,
}

define_class!(
    #[unsafe(super(UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AccessKitExampleSceneDelegate"]
    #[ivars = SceneDelegateIvars]
    struct SceneDelegate;

    impl SceneDelegate {
        // UIKit creates the scene delegate.
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Option<Retained<Self>> {
            let this = this.set_ivars(SceneDelegateIvars {
                window: RefCell::new(None),
            });
            unsafe { msg_send![super(this), init] }
        }
    }

    unsafe impl NSObjectProtocol for SceneDelegate {}

    unsafe impl UISceneDelegate for SceneDelegate {
        #[unsafe(method(scene:willConnectToSession:options:))]
        fn scene_will_connect(
            &self,
            scene: &UIScene,
            _session: &UISceneSession,
            _connection_options: &UISceneConnectionOptions,
        ) {
            let mtm = MainThreadMarker::from(self);
            if !scene.isKindOfClass(UIWindowScene::class()) {
                return;
            }
            // SAFETY: We just checked that the scene is a `UIWindowScene`.
            let window_scene =
                unsafe { &*(scene as *const UIScene as *const UIWindowScene) };

            let window = UIWindow::initWithWindowScene(mtm.alloc(), window_scene);
            let view_controller = UIViewController::init(mtm.alloc());
            window.setRootViewController(Some(&view_controller));
            window.makeKeyAndVisible();

            let root_view = view_controller.view().expect("view controller has no view");
            let content_view = ContentView::new(mtm, root_view.bounds());
            root_view.addSubview(&content_view);

            *self.ivars().window.borrow_mut() = Some(window);
        }
    }

    unsafe impl UIWindowSceneDelegate for SceneDelegate {}
);

fn main() {
    let mtm = MainThreadMarker::new().unwrap();
    // UIKit looks the delegate classes up by name, so they must be registered
    // first.
    let delegate_class_name = NSString::from_class(AppDelegate::class());
    let _ = SceneDelegate::class();
    UIApplication::main(None, Some(&delegate_class_name), mtm);
}
