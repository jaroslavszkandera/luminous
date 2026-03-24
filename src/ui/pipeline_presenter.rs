use crate::AppController;
use crate::MainWindow;
use crate::pipeline::{StepFactory, run_pipeline_on_selection};
use crate::{PipelineStep, PipelineStepKind, RotateAngle};
use slint::{Model, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

pub fn register(window: &MainWindow, c: Rc<RefCell<AppController>>, factory: Arc<StepFactory>) {
    window.set_pipeline_steps(Rc::new(VecModel::<PipelineStep>::default()).into());

    let c1 = c.clone();
    window.on_pipeline_add_step(move |kind| {
        let Some(ui) = c1.borrow().window_weak.upgrade() else {
            return;
        };
        let model = ui.get_pipeline_steps();
        let vec_model = model
            .as_any()
            .downcast_ref::<VecModel<PipelineStep>>()
            .expect("pipeline_steps must be a VecModel");
        let new_step = match kind {
            PipelineStepKind::Rotate => PipelineStep {
                kind,
                rotate_angle: RotateAngle::R90,
                blur_sigma: 0.0,
                brighten_value: 0,
            },
            PipelineStepKind::GaussianBlur => PipelineStep {
                kind,
                rotate_angle: RotateAngle::R90,
                blur_sigma: 1.0,
                brighten_value: 0,
            },
            PipelineStepKind::Brighten => PipelineStep {
                kind,
                rotate_angle: RotateAngle::R90,
                blur_sigma: 0.0,
                brighten_value: 10,
            },
        };
        vec_model.push(new_step);
    });

    let c2 = c.clone();
    window.on_pipeline_remove_step(move |index| {
        let Some(ui) = c2.borrow().window_weak.upgrade() else {
            return;
        };
        let model = ui.get_pipeline_steps();
        let vec_model = model
            .as_any()
            .downcast_ref::<VecModel<PipelineStep>>()
            .expect("pipeline_steps must be a VecModel");
        if (index as usize) < vec_model.row_count() {
            vec_model.remove(index as usize);
        }
    });

    let c3 = c.clone();
    window.on_pipeline_update_rotate(move |index, angle| {
        let Some(ui) = c3.borrow().window_weak.upgrade() else {
            return;
        };
        let model = ui.get_pipeline_steps();
        let vec_model = model
            .as_any()
            .downcast_ref::<VecModel<PipelineStep>>()
            .expect("pipeline_steps must be a VecModel");
        if let Some(mut step) = vec_model.row_data(index as usize) {
            step.rotate_angle = angle;
            vec_model.set_row_data(index as usize, step);
        }
    });

    let c4 = c.clone();
    window.on_pipeline_update_sigma(move |index, sigma| {
        let Some(ui) = c4.borrow().window_weak.upgrade() else {
            return;
        };
        let model = ui.get_pipeline_steps();
        let vec_model = model
            .as_any()
            .downcast_ref::<VecModel<PipelineStep>>()
            .expect("pipeline_steps must be a VecModel");
        if let Some(mut step) = vec_model.row_data(index as usize) {
            step.blur_sigma = sigma;
            vec_model.set_row_data(index as usize, step);
        }
    });

    let c5 = c.clone();
    window.on_pipeline_update_brighten(move |index, value| {
        let Some(ui) = c5.borrow().window_weak.upgrade() else {
            return;
        };
        let model = ui.get_pipeline_steps();
        let vec_model = model
            .as_any()
            .downcast_ref::<VecModel<PipelineStep>>()
            .expect("pipeline_steps must be a VecModel");
        if let Some(mut step) = vec_model.row_data(index as usize) {
            step.brighten_value = value;
            vec_model.set_row_data(index as usize, step);
        }
    });

    let c6 = c.clone();
    window.on_pipeline_run(move || {
        let (paths, weak_ui) = {
            let c_ref = c6.borrow();
            let paths = c_ref.collect_selected_paths();
            let weak = c_ref.window_weak.clone();
            (paths, weak)
        };
        if paths.is_empty() {
            log::info!("Pipeline: no images selected");
            return;
        }
        let steps: Vec<PipelineStep> = {
            let Some(ui) = weak_ui.upgrade() else { return };
            let model = ui.get_pipeline_steps();
            (0..model.row_count())
                .filter_map(|i| model.row_data(i))
                .collect()
        };
        run_pipeline_on_selection(paths, steps, factory.clone());
        slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak_ui.upgrade() {
                ui.invoke_return_focus();
            }
        })
        .unwrap();
    });
}
