mod protocol;
mod runtime;
pub mod scheduler;
pub mod store;

use runtime::{AppState, Candidate, Snapshot};
use std::sync::Arc;
use tauri::{Manager, State};
use tauri_plugin_opener::OpenerExt;

type Shared = Arc<AppState>;

#[tauri::command]
async fn get_snapshot(state: State<'_, Shared>) -> Result<Snapshot, String> {
    Ok(state.snapshot.read().await.clone())
}

#[tauri::command]
async fn list_candidates(state: State<'_, Shared>) -> Result<Vec<Candidate>, String> {
    state.runtime.lock().await.list_candidates().await
}

#[tauri::command]
async fn add_task(state: State<'_, Shared>, thread_id: String) -> Result<(), String> {
    let mut runtime = state.runtime.lock().await;
    runtime.add_task(thread_id).await?;
    state.publish(&mut runtime).await;
    Ok(())
}

#[tauri::command]
async fn set_task_enabled(
    state: State<'_, Shared>,
    thread_id: String,
    enabled: bool,
) -> Result<(), String> {
    let mut runtime = state.runtime.lock().await;
    runtime.set_enabled(&thread_id, enabled)?;
    state.publish(&mut runtime).await;
    Ok(())
}

#[tauri::command]
async fn set_paused(state: State<'_, Shared>, paused: bool) -> Result<(), String> {
    let mut runtime = state.runtime.lock().await;
    runtime.set_paused(paused)?;
    state.publish(&mut runtime).await;
    Ok(())
}

#[tauri::command]
async fn remove_task(state: State<'_, Shared>, thread_id: String) -> Result<(), String> {
    let mut runtime = state.runtime.lock().await;
    runtime.remove_task(&thread_id)?;
    state.publish(&mut runtime).await;
    Ok(())
}

#[tauri::command]
async fn reconnect(state: State<'_, Shared>) -> Result<(), String> {
    state.runtime.lock().await.reconnect()?;
    state.refresh().await;
    Ok(())
}

#[tauri::command]
async fn login(app: tauri::AppHandle, state: State<'_, Shared>) -> Result<(), String> {
    let url = state.runtime.lock().await.login_url().await?;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|_| "无法打开系统浏览器".into())
}

#[tauri::command]
fn open_download(app: tauri::AppHandle) -> Result<(), String> {
    app.opener()
        .open_url("https://learn.chatgpt.com/docs/cli", None::<&str>)
        .map_err(|_| "无法打开官方安装说明".into())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            show_window(app)
        }))
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            list_candidates,
            add_task,
            set_task_enabled,
            set_paused,
            remove_task,
            reconnect,
            login,
            open_download
        ])
        .setup(|app| {
            let directory = app.path().app_data_dir()?;
            std::fs::create_dir_all(&directory)?;
            let state =
                AppState::open(&directory.join("autoresume.db")).map_err(std::io::Error::other)?;
            app.manage(state.clone());
            let show = tauri::menu::MenuItem::with_id(
                app,
                "show",
                "打开 Codex AutoResume",
                true,
                None::<&str>,
            )?;
            let pause =
                tauri::menu::MenuItem::with_id(app, "pause", "暂停全部监测", true, None::<&str>)?;
            let quit = tauri::menu::MenuItem::with_id(
                app,
                "quit",
                "退出（将停止后台监测）",
                true,
                None::<&str>,
            )?;
            let menu = tauri::menu::Menu::with_items(app, &[&show, &pause, &quit])?;
            let mut tray = tauri::tray::TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("Codex AutoResume · 开发预览")
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_window(app),
                    "pause" => {
                        let state = app.state::<Shared>().inner().clone();
                        tauri::async_runtime::spawn(async move {
                            let mut runtime = state.runtime.lock().await;
                            if runtime.set_paused(true).is_ok() {
                                state.publish(&mut runtime).await;
                            }
                        });
                    }
                    "quit" => app.exit(0),
                    _ => {}
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            tauri::async_runtime::spawn(async move {
                // Every tick obtains fresh server state, including after sleep. Never
                // replay timer firings or missed quota windows on wake.
                loop {
                    state.refresh().await;
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.hide().is_ok() {
                    api.prevent_close();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("无法启动 Codex AutoResume")
        .run(|app, event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                show_window(app);
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        });
}

fn show_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}
