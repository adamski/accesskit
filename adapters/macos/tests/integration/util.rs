// Copyright 2026 The AccessKit Authors. All rights reserved.
// Licensed under the Apache License, Version 2.0 (found in
// the LICENSE-APACHE file) or the MIT license (found in
// the LICENSE-MIT file), at your option.

use std::{cell::Cell, rc::Rc};

use accesskit::{
    ActionHandler, ActionRequest, ActivationHandler, Node, NodeId, Rect, Role, TreeId, TreeInfo,
    TreeUpdate,
};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_app_kit::NSView;

#[derive(Default)]
pub struct HandlerEvents {
    pub tree_requested: Cell<bool>,
    pub dropped: Cell<bool>,
}

pub struct NoTree(pub Rc<HandlerEvents>);

impl ActivationHandler for NoTree {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        self.0.tree_requested.set(true);
        None
    }
}

impl Drop for NoTree {
    fn drop(&mut self) {
        self.0.dropped.set(true);
    }
}

pub struct WindowTree;

impl ActivationHandler for WindowTree {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        let mut window = Node::new(Role::Window);
        window.set_bounds(Rect::new(0.0, 0.0, 100.0, 100.0));
        Some(TreeUpdate {
            nodes: vec![(NodeId(0), window)],
            tree: Some(TreeInfo::new(NodeId(0))),
            tree_id: TreeId::ROOT,
            focus: NodeId(0),
        })
    }
}

pub struct NoActions;

impl ActionHandler for NoActions {
    fn do_action(&mut self, _request: ActionRequest) {}
}

pub fn new_view() -> Retained<NSView> {
    unsafe { msg_send![class!(NSView), new] }
}

// KVO overrides the `class` method to hide its own subclass.
pub fn raw_class_name(view: &NSView) -> String {
    let object: &AnyObject = view;
    object.class().name().to_string_lossy().into_owned()
}

/// The class name without the random `_<16 hex digits>` suffix that
/// makes our subclass names unique.
pub fn class_name(view: &NSView) -> String {
    let name = raw_class_name(view);
    match name.rsplit_once('_') {
        Some((base, suffix))
            if suffix.len() == 16 && suffix.bytes().all(|b| b.is_ascii_hexdigit()) =>
        {
            base.to_owned()
        }
        _ => name,
    }
}
