//! Tauri bridge for the doctor checks (`tt-doctor`). One async command; the
//! ~10 subprocess probes run on a blocking worker so the UI stays live while
//! they spin.

#[tauri::command]
pub async fn doctor_run() -> Result<tt_doctor::DoctorReport, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let mut report = tt_doctor::run_report();
        // App-only check: it inspects the VT parser linked into *this*
        // process, so it can't live inside run_report (the `tt` CLI has no
        // VT engine and would report nothing meaningful).
        report.result.tools.push(tt_doctor::check_vt_parser(tt_vt::parser_optimize_mode()));
        report
    })
    .await
    .map_err(|e| format!("doctor task failed: {e}"))
}
