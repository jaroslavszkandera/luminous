use crate::AppController;
use crate::MainWindow;
use crate::pipeline::{StepFactory, run_pipeline_on_selection};
use crate::{Channel, FlipDirection, PipelineStep, PipelineStepKind, RotateAngle};
use slint::{Model, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

pub fn register(window: &MainWindow, c: Rc<RefCell<AppController>>, factory: Arc<StepFactory>) {
    window.set_pipeline_steps(Rc::new(VecModel::<PipelineStep>::default()).into());

    let acc = c.clone();
    window.on_pipeline_add_step(move |kind| {
        let Some(ui) = acc.borrow().window_weak.upgrade() else {
            return;
        };
        let model = ui.get_pipeline_steps();
        let vec_model = model
            .as_any()
            .downcast_ref::<VecModel<PipelineStep>>()
            .unwrap();

        let mut new_step = PipelineStep {
            kind,
            rotate_angle: RotateAngle::R90,
            blur_sigma: 1.0,
            brighten_value: 0,
            resize_width: 224,
            resize_height: 224,
            flip_direction: FlipDirection::Horizontal,
            extract_channel: Channel::Gray,
        };

        match kind {
            PipelineStepKind::Brighten => new_step.brighten_value = 10,
            _ => {}
        }
        vec_model.push(new_step);
    });

    let acc = c.clone();
    window.on_pipeline_remove_step(move |index| {
        let Some(ui) = acc.borrow().window_weak.upgrade() else {
            return;
        };
        let model = ui.get_pipeline_steps();
        let vec_model = model
            .as_any()
            .downcast_ref::<VecModel<PipelineStep>>()
            .unwrap();
        if (index as usize) < vec_model.row_count() {
            vec_model.remove(index as usize);
        }
    });

    let acc = c.clone();
    window.on_pipeline_move_step(move |from, to| {
        let Some(ui) = acc.borrow().window_weak.upgrade() else {
            return;
        };
        let model = ui.get_pipeline_steps();
        let vec_model = model
            .as_any()
            .downcast_ref::<VecModel<PipelineStep>>()
            .unwrap();
        let len = vec_model.row_count() as i32;
        if from < 0 || from >= len || to < 0 || to >= len {
            return;
        }
        let step = vec_model.row_data(from as usize).unwrap();
        vec_model.remove(from as usize);
        vec_model.insert(to as usize, step);
    });

    let acc = c.clone();
    window.on_pipeline_update_step(move |index, step| {
        let Some(ui) = acc.borrow().window_weak.upgrade() else {
            return;
        };
        let model = ui.get_pipeline_steps();
        let vec_model = model
            .as_any()
            .downcast_ref::<VecModel<PipelineStep>>()
            .unwrap();
        vec_model.set_row_data(index as usize, step);
    });

    let acc = c.clone();
    window.on_pipeline_run(move |encode_extension| {
        let (paths, weak_ui, plugin_manager) = {
            let c_ref = acc.borrow();
            let paths = c_ref.collect_selected_paths();
            let weak = c_ref.window_weak.clone();
            let plugin_manager = c_ref.loader.plugin_manager.clone();
            (paths, weak, plugin_manager)
        };
        if paths.is_empty() {
            return;
        }

        let steps: Vec<PipelineStep> = {
            let Some(ui) = weak_ui.upgrade() else { return };
            let model = ui.get_pipeline_steps();
            (0..model.row_count())
                .filter_map(|i| model.row_data(i))
                .collect()
        };

        run_pipeline_on_selection(
            paths,
            steps,
            factory.clone(),
            encode_extension.to_string(),
            plugin_manager,
            weak_ui.clone(),
        );
        slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak_ui.upgrade() {
                ui.invoke_return_focus();
            }
        })
        .unwrap();
    });
}
