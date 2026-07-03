//! Towles Tool desktop app (Tauri 2). A minimal shell hosting the React
//! frontend; Tauri commands will be added as the client screens get wired to
//! real data (the old AgentBoard bridge moved to the tmux-mode `ttr
//! agentboard` CLI).

pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running Towles Tool application");
}
