//! Tauri bridge for the doctor checks (`tt-doctor`). One async command; the
//! ~10 subprocess probes run on a blocking worker so the UI stays live while
//! they spin.

#[tauri::command]
pub async fn doctor_run() -> Result<tt_doctor::DoctorReport, String> {
    tauri::async_runtime::spawn_blocking(tt_doctor::run_report)
        .await
        .map_err(|e| format!("doctor task failed: {e}"))
}
