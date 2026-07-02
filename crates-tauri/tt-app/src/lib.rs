#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {name}! Welcome to Towles Tool.")
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![greet])
        .run(tauri::generate_context!())
        .expect("error while running Towles Tool application");
}
