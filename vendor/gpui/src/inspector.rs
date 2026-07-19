/// A unique identifier for an element that can be inspected.
#[derive(Debug, Eq, PartialEq, Hash, Clone)]
pub struct InspectorElementId {
    /// Stable part of the ID.
    #[cfg(any(feature = "inspector", debug_assertions))]
    pub path: std::rc::Rc<InspectorElementPath>,
    /// Disambiguates elements that have the same path.
    #[cfg(any(feature = "inspector", debug_assertions))]
    pub instance_id: usize,
}

impl Into<InspectorElementId> for &InspectorElementId {
    fn into(self) -> InspectorElementId {
        self.clone()
    }
}

#[cfg(any(feature = "inspector", debug_assertions))]
pub use conditional::*;

#[cfg(any(feature = "inspector", debug_assertions))]
mod conditional {
    use super::*;
    use crate::{AnyElement, App, Bounds, Context, Empty, IntoElement, Pixels, Render, Window};
    use collections::{FxHashMap, FxHashSet};
    use std::any::{Any, TypeId};

    /// `GlobalElementId` qualified by source location of element construction.
    #[derive(Debug, Eq, PartialEq, Hash)]
    pub struct InspectorElementPath {
        /// The path to the nearest ancestor element that has an `ElementId`.
        #[cfg(any(feature = "inspector", debug_assertions))]
        pub global_id: crate::GlobalElementId,
        /// Source location where this element was constructed.
        #[cfg(any(feature = "inspector", debug_assertions))]
        pub source_location: &'static std::panic::Location<'static>,
    }

    impl Clone for InspectorElementPath {
        fn clone(&self) -> Self {
            Self {
                global_id: crate::GlobalElementId(self.global_id.0.clone()),
                source_location: self.source_location,
            }
        }
    }

    impl Into<InspectorElementPath> for &InspectorElementPath {
        fn into(self) -> InspectorElementPath {
            self.clone()
        }
    }

    /// A node in the most recently rendered inspectable element tree.
    ///
    /// Unlike a browser DOM node, this is a snapshot. GPUI rebuilds its element tree for every
    /// frame, and the inspector replaces this list after the target window finishes rendering.
    #[derive(Clone, Debug, PartialEq)]
    pub struct InspectorElementNode {
        /// ID used to select this element and associate its inspector state.
        pub id: InspectorElementId,
        /// Nearest inspectable parent in the rendered element tree.
        pub parent: Option<InspectorElementId>,
        /// Concrete Rust element type used for this frame.
        pub element_type: &'static str,
        /// Actual bounds computed during prepaint.
        pub bounds: Bounds<Pixels>,
    }

    /// Function set on `App` to render the inspector UI.
    pub type InspectorRenderer =
        Box<dyn Fn(&mut Inspector, &mut Window, &mut Context<Inspector>) -> AnyElement>;

    /// Manages inspector state - which element is currently selected and whether the inspector is
    /// in picking mode.
    pub struct Inspector {
        active_element: Option<InspectedElement>,
        pub(crate) pick_depth: Option<f32>,
        element_tree: Vec<InspectorElementNode>,
        tree_revision: u64,
        collapsed_elements: FxHashSet<InspectorElementId>,
    }

    struct InspectedElement {
        id: InspectorElementId,
        states: FxHashMap<TypeId, Box<dyn Any>>,
    }

    impl InspectedElement {
        fn new(id: InspectorElementId) -> Self {
            InspectedElement {
                id,
                states: FxHashMap::default(),
            }
        }
    }

    impl Inspector {
        pub(crate) fn new() -> Self {
            Self {
                active_element: None,
                pick_depth: Some(0.0),
                element_tree: Vec::new(),
                tree_revision: 0,
                collapsed_elements: FxHashSet::default(),
            }
        }

        pub(crate) fn select(&mut self, id: InspectorElementId, window: &mut Window) {
            self.set_active_element_id(id, window);
            self.pick_depth = None;
        }

        pub(crate) fn hover(&mut self, id: InspectorElementId, window: &mut Window) {
            if self.is_picking() {
                let changed = self.set_active_element_id(id, window);
                if changed {
                    self.pick_depth = Some(0.0);
                }
            }
        }

        pub(crate) fn set_active_element_id(
            &mut self,
            id: InspectorElementId,
            window: &mut Window,
        ) -> bool {
            let changed = Some(&id) != self.active_element_id();
            if changed {
                self.active_element = Some(InspectedElement::new(id));
                window.refresh();
            }
            changed
        }

        /// ID of the currently hovered or selected element.
        pub fn active_element_id(&self) -> Option<&InspectorElementId> {
            self.active_element.as_ref().map(|e| &e.id)
        }

        /// The inspectable element tree from the target window's most recently rendered frame.
        pub fn element_tree(&self) -> &[InspectorElementNode] {
            &self.element_tree
        }

        /// Monotonically increasing revision of the element tree snapshot.
        pub fn tree_revision(&self) -> u64 {
            self.tree_revision
        }

        /// Returns whether a tree node is collapsed in the inspector UI.
        pub fn is_tree_node_collapsed(&self, id: &InspectorElementId) -> bool {
            self.collapsed_elements.contains(id)
        }

        /// Toggles whether a tree node is collapsed in the inspector UI.
        pub fn toggle_tree_node(&mut self, id: InspectorElementId) {
            if !self.collapsed_elements.remove(&id) {
                self.collapsed_elements.insert(id);
            }
        }

        pub(crate) fn replace_element_tree(&mut self, nodes: Vec<InspectorElementNode>) -> bool {
            if self.element_tree == nodes {
                return false;
            }

            self.element_tree = nodes;
            self.tree_revision = self.tree_revision.wrapping_add(1);
            self.collapsed_elements
                .retain(|id| self.element_tree.iter().any(|node| &node.id == id));

            if self
                .active_element_id()
                .is_some_and(|active| !self.element_tree.iter().any(|node| &node.id == active))
            {
                self.active_element = None;
            }
            true
        }

        pub(crate) fn with_active_element_state<T: 'static, R>(
            &mut self,
            window: &mut Window,
            f: impl FnOnce(&mut Option<T>, &mut Window) -> R,
        ) -> R {
            let Some(active_element) = &mut self.active_element else {
                return f(&mut None, window);
            };

            let type_id = TypeId::of::<T>();
            let mut inspector_state = active_element
                .states
                .remove(&type_id)
                .map(|state| *state.downcast().unwrap());

            let result = f(&mut inspector_state, window);

            if let Some(inspector_state) = inspector_state {
                active_element
                    .states
                    .insert(type_id, Box::new(inspector_state));
            }

            result
        }

        /// Starts element picking mode, allowing the user to select elements by clicking.
        pub fn start_picking(&mut self) {
            self.pick_depth = Some(0.0);
        }

        /// Returns whether the inspector is currently in picking mode.
        pub fn is_picking(&self) -> bool {
            self.pick_depth.is_some()
        }

        /// Renders elements for all registered inspector states of the active inspector element.
        pub fn render_inspector_states(
            &mut self,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) -> Vec<AnyElement> {
            let mut elements = Vec::new();
            if let Some(active_element) = self.active_element.take() {
                for (type_id, state) in &active_element.states {
                    if let Some(render_inspector) = cx
                        .inspector_element_registry
                        .renderers_by_type_id
                        .remove(type_id)
                    {
                        let mut element = (render_inspector)(
                            active_element.id.clone(),
                            state.as_ref(),
                            window,
                            cx,
                        );
                        elements.push(element);
                        cx.inspector_element_registry
                            .renderers_by_type_id
                            .insert(*type_id, render_inspector);
                    }
                }

                self.active_element = Some(active_element);
            }

            elements
        }
    }

    impl Render for Inspector {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            if let Some(inspector_renderer) = cx.inspector_renderer.take() {
                let result = inspector_renderer(self, window, cx);
                cx.inspector_renderer = Some(inspector_renderer);
                result
            } else {
                Empty.into_any_element()
            }
        }
    }

    #[derive(Default)]
    pub(crate) struct InspectorElementRegistry {
        renderers_by_type_id: FxHashMap<
            TypeId,
            Box<dyn Fn(InspectorElementId, &dyn Any, &mut Window, &mut App) -> AnyElement>,
        >,
    }

    impl InspectorElementRegistry {
        pub fn register<T: 'static, R: IntoElement>(
            &mut self,
            f: impl 'static + Fn(InspectorElementId, &T, &mut Window, &mut App) -> R,
        ) {
            self.renderers_by_type_id.insert(
                TypeId::of::<T>(),
                Box::new(move |id, value, window, cx| {
                    let value = value.downcast_ref().unwrap();
                    f(id, value, window, cx).into_any_element()
                }),
            );
        }
    }
}

/// Provides definitions used by `#[derive_inspector_reflection]`.
#[cfg(any(feature = "inspector", debug_assertions))]
pub mod inspector_reflection {
    use std::any::Any;

    /// Reification of a function that has the signature `fn some_fn(T) -> T`. Provides the name,
    /// documentation, and ability to invoke the function.
    #[derive(Clone, Copy)]
    pub struct FunctionReflection<T> {
        /// The name of the function
        pub name: &'static str,
        /// The method
        pub function: fn(Box<dyn Any>) -> Box<dyn Any>,
        /// Documentation for the function
        pub documentation: Option<&'static str>,
        /// `PhantomData` for the type of the argument and result
        pub _type: std::marker::PhantomData<T>,
    }

    impl<T: 'static> FunctionReflection<T> {
        /// Invoke this method on a value and return the result.
        pub fn invoke(&self, value: T) -> T {
            let boxed = Box::new(value) as Box<dyn Any>;
            let result = (self.function)(boxed);
            *result
                .downcast::<T>()
                .expect("Type mismatch in reflection invoke")
        }
    }
}
