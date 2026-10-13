use gpui_component::ActiveTheme;
use crate::components::ui::pane_grid::renderer::PaneGridRenderer;
use crate::data::models::SettingsData;
use crate::data::pane_tree::PaneNode;
use gpui::*;


pub struct GlobalHomeView {
    pub view: gpui::WeakEntity<HomePage>,
}
impl gpui::Global for GlobalHomeView {}

pub enum HomePageEvent {
    LayoutModified,
}

pub struct HomePage {
    pub pane_grid: Entity<PaneGridRenderer>,
}

impl HomePage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        if !cx.has_global::<crate::components::ui::pane_grid::renderer::PendingGridEvents>() {
            cx.set_global(crate::components::ui::pane_grid::renderer::PendingGridEvents::default());
        }
        
        let view = cx.weak_entity();
        cx.set_global(GlobalHomeView { view });
        

        
        Self {
            pane_grid: cx.new(|_cx| PaneGridRenderer::new()),
        }
    }

    pub fn request_module_load(
        &mut self,
        module_json: serde_json::Value,
        _module_id: String,
        cx: &mut Context<Self>,
    ) {
        if let Ok(tree) = serde_json::from_value::<PaneNode>(module_json.clone()) {
            tracing::info!("Successfully parsed PaneNode tree!");
            self.pane_grid.update(cx, |grid, cx| {
                grid.set_tree(tree, cx);
            });
            // Subscribe to grid events
            cx.subscribe(
                &self.pane_grid,
                |this: &mut HomePage,
                 _,
                 event: &crate::components::ui::pane_grid::renderer::PaneGridEvent,
                 cx| {
                    this.handle_grid_event(event, cx);
                },
            )
            .detach();
        } else {
            tracing::warn!("Failed to parse module JSON into PaneNode tree.");
        }
    }

    fn handle_grid_event(
        &mut self,
        event: &crate::components::ui::pane_grid::renderer::PaneGridEvent,
        cx: &mut Context<Self>,
    ) {
        use crate::components::ui::pane_grid::renderer::PaneGridEvent;

        let mut modified = false;
        self.pane_grid.update(cx, |grid, cx| {
            if let Some(mut tree) = grid.tree.clone() {
                match event {
                    PaneGridEvent::SwapPanes { id_a, id_b } => {
                        if tree.swap_data(id_a, id_b) {
                            grid.set_tree(tree, cx);
                            modified = true;
                        }
                    }
                    PaneGridEvent::DropSplit {
                        target_pane_id,
                        dragged_pane_id,
                        region,
                    } => {
                        println!(
                            "DropSplit: target={}, dragged={}, region={}",
                            target_pane_id, dragged_pane_id, region
                        );
                        if target_pane_id != dragged_pane_id {
                            let mut dragged_node_opt = None;
                            tree.modify_node(dragged_pane_id, &mut |node| {
                                dragged_node_opt = Some(node.clone());
                            });

                            if let Some(PaneNode::Pane {
                                data: dragged_data, ..
                            }) = dragged_node_opt
                            {
                                if let Some(new_tree) = tree.clone().remove_node(dragged_pane_id) {
                                    let mut updated_tree = new_tree;
                                    updated_tree.modify_node(target_pane_id, &mut |node| {
                                        if let PaneNode::Pane { id, data } = node.clone() {
                                            let target_leaf = PaneNode::Pane {
                                                id: id.clone(),
                                                data: data.clone(),
                                            };
                                            let dragged_leaf = PaneNode::Pane {
                                                id: dragged_pane_id.clone(),
                                                data: dragged_data.clone(),
                                            };

                                            let (axis, a, b) = match region.as_str() {
                                                "top" => (
                                                    crate::data::pane_tree::Axis::Horizontal,
                                                    dragged_leaf,
                                                    target_leaf,
                                                ),
                                                "bottom" => (
                                                    crate::data::pane_tree::Axis::Horizontal,
                                                    target_leaf,
                                                    dragged_leaf,
                                                ),
                                                "left" => (
                                                    crate::data::pane_tree::Axis::Vertical,
                                                    dragged_leaf,
                                                    target_leaf,
                                                ),
                                                "right" => (
                                                    crate::data::pane_tree::Axis::Vertical,
                                                    target_leaf,
                                                    dragged_leaf,
                                                ),
                                                _ => (
                                                    crate::data::pane_tree::Axis::Horizontal,
                                                    target_leaf,
                                                    dragged_leaf,
                                                ),
                                            };

                                            *node = PaneNode::Split {
                                                id: PaneNode::generate_id(),
                                                axis,
                                                ratio: 0.5,
                                                a: Box::new(a),
                                                b: Box::new(b),
                                            };
                                        }
                                    });
                                    grid.set_tree(updated_tree, cx);
                                    modified = true;
                                }
                            }
                        }
                    }
                    PaneGridEvent::SetActivePane { pane_id } => {
                        grid.active_pane_id = Some(pane_id.clone());
                        cx.notify();
                    }
                    PaneGridEvent::SetModuleType {
                        pane_id,
                        module_type,
                    } => {
                        tree.modify_node(pane_id, &mut |node| {
                            if let PaneNode::Pane { data, .. } = node {
                                data.module_type = Some(module_type.clone());
                            }
                        });
                        grid.set_tree(tree, cx);
                        modified = true;
                    }
                    PaneGridEvent::UpdateModuleConfig { pane_id, config } => {
                        tree.modify_node(pane_id, &mut |node| {
                            if let PaneNode::Pane { data, .. } = node {
                                if let (Some(current), Some(update)) =
                                    (data.config.as_object_mut(), config.as_object())
                                {
                                    current.extend(update.clone());
                                } else {
                                    data.config = config.clone();
                                }
                            }
                        });
                        grid.set_tree(tree, cx);
                        modified = true;
                    }
                    PaneGridEvent::SplitPane { pane_id, axis } => {
                        tree.modify_node(pane_id, &mut |node| {
                            if let PaneNode::Pane { id, data } = node.clone() {
                                let new_a = PaneNode::Pane {
                                    id: id.clone(),
                                    data: data.clone(),
                                };
                                let new_b = PaneNode::Pane {
                                    id: PaneNode::generate_id(),
                                    data: crate::data::pane_tree::ModuleData::default(),
                                };
                                *node = PaneNode::Split {
                                    id: PaneNode::generate_id(),
                                    axis: axis.clone(),
                                    ratio: 0.5,
                                    a: Box::new(new_a),
                                    b: Box::new(new_b),
                                };
                            }
                        });
                        grid.set_tree(tree, cx);
                        modified = true;
                    }
                    PaneGridEvent::ClosePane { pane_id } => {
                        if let Some(new_tree) = tree.remove_node(pane_id) {
                            grid.set_tree(new_tree, cx);
                            modified = true;
                        }
                    }
                    PaneGridEvent::ResizeSplit { split_id, ratio } => {
                        tree.modify_node(split_id, &mut |node| {
                            if let PaneNode::Split { ratio: r, .. } = node {
                                *r = *ratio;
                            }
                        });
                        grid.set_tree(tree.clone(), cx);
                        modified = false; // don't save to DB on every tick!
                    }
                    PaneGridEvent::ToggleMaximize { pane_id } => { grid.maximized_pane = if grid.maximized_pane.as_ref() == Some(pane_id) { None } else { Some(pane_id.clone()) }; cx.notify(); }, PaneGridEvent::SplitMatrix { pane_id, rows, cols } => {
                        tree.split_matrix(pane_id, *rows, *cols);
                        grid.set_tree(tree, cx);
                        modified = true;
                    }
                    PaneGridEvent::EndResize => {
                        modified = true; // save to DB when user drops the divider
                    }
                }
            }
        });

        if modified {
            cx.emit(HomePageEvent::LayoutModified);
        }
    }


    pub fn handle_pane_event(&mut self, event: &crate::components::ui::pane_grid::renderer::PaneGridEvent, cx: &mut Context<Self>) {
        self.pane_grid.update(cx, |grid, cx| {
            if let Some(mut tree) = grid.tree.clone() {
                let mut modified = false;
                match event {
                    crate::components::ui::pane_grid::renderer::PaneGridEvent::SetModuleType { pane_id, module_type } => {
                        let default_timezone = grid.timezone.clone();
                        tree.modify_node(pane_id, &mut |node| {
                            if let PaneNode::Pane { data, .. } = node {
                                let mut config = data.config.as_object().cloned().unwrap_or_default();
                                let defaults = match module_type.as_str() {
                                    "time" => serde_json::json!({ "timezone": default_timezone }),
                                    "ticker" => serde_json::json!({ "exchange": "binance", "symbol": "BTC/USDT", "market": "spot" }),
                                    "chart" => serde_json::json!({ "exchange": "binance", "symbol": "BTC/USDT", "interval": "1h", "timeframe": "1h", "type": "candle_solid", "market": "spot" }),
                                    "orderbook" => serde_json::json!({ "exchange": "binance", "symbol": "BTC/USDT", "market": "spot", "limit": 20 }),
                                    "depth" => serde_json::json!({ "exchange": "binance", "symbol": "BTC/USDT", "market": "spot" }),
                                    "editor" => serde_json::json!({ "script": "", "linkedModules": [] }),
                                    "newsfeed" => serde_json::json!({ "limit": 50, "priority": "latest", "filter": "" }),
                                    _ => serde_json::json!({}),
                                };
                                if let Some(defaults) = defaults.as_object() {
                                    for (key, value) in defaults {
                                        if !config.contains_key(key) || config.get(key).is_some_and(|value| value.is_null() || value.as_str() == Some("")) {
                                            config.insert(key.clone(), value.clone());
                                        }
                                    }
                                }
                                data.module_type = Some(module_type.clone());
                                data.config = serde_json::Value::Object(config);
                            }
                        });
                        modified = true;
                    }
                    crate::components::ui::pane_grid::renderer::PaneGridEvent::SplitPane { pane_id, axis } => {
                        tree.modify_node(pane_id, &mut |node| {
                            if let PaneNode::Pane { id, data } = node.clone() {
                                *node = PaneNode::Split {
                                    id: PaneNode::generate_id(),
                                    axis: axis.clone(),
                                    ratio: 0.5,
                                    a: Box::new(PaneNode::Pane { id: id.clone(), data: data.clone() }),
                                    b: Box::new(PaneNode::Pane { id: PaneNode::generate_id(), data: crate::data::pane_tree::ModuleData::default() }),
                                };
                            }
                        });
                        modified = true;
                    }
                    crate::components::ui::pane_grid::renderer::PaneGridEvent::SplitMatrix { pane_id, rows, cols } => {
                        tree.split_matrix(pane_id, *rows, *cols);
                        modified = true;
                    }
                    crate::components::ui::pane_grid::renderer::PaneGridEvent::UpdateModuleConfig { pane_id, config } => {
                        tree.modify_node(pane_id, &mut |node| {
                            if let PaneNode::Pane { data, .. } = node {
                                if let (Some(current), Some(update)) =
                                    (data.config.as_object_mut(), config.as_object())
                                {
                                    current.extend(update.clone());
                                } else {
                                    data.config = config.clone();
                                }
                            }
                        });
                        modified = true;
                    }
                    crate::components::ui::pane_grid::renderer::PaneGridEvent::ToggleMaximize { pane_id } => {
                        grid.maximized_pane = if grid.maximized_pane.as_ref() == Some(pane_id) {
                            None
                        } else {
                            Some(pane_id.clone())
                        };
                        cx.notify();
                    }
                    _ => {}
                }
                if modified {
                    grid.set_tree(tree, cx);
                }
            }
        });
        cx.emit(HomePageEvent::LayoutModified);
    }

    pub fn apply_settings(&mut self, settings: SettingsData, cx: &mut Context<Self>) {
        let timezone = settings.timezone.unwrap_or_else(|| "UTC".into());
        self.pane_grid.update(cx, |grid, cx| {
            grid.timezone = timezone.clone();
            for state in grid.chart_views.borrow().values() {
                let timezone = timezone.clone();
                state.entity.update(cx, |chart, _| chart.set_timezone(timezone));
            }
            for state in grid.newsfeed_views.borrow().values() {
                let timezone = timezone.clone();
                state.entity.update(cx, |panel, _| panel.set_timezone(timezone));
            }
            for state in grid.clock_views.borrow().values() {
                let timezone = timezone.clone();
                state.entity.update(cx, |panel, _| panel.set_timezone(timezone));
            }
            cx.notify();
        });
    }
}

impl Render for HomePage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().colors.background) // Will be overridden by PaneGrid background
            .child(self.pane_grid.clone())
    }
}

impl gpui::EventEmitter<HomePageEvent> for HomePage {}
