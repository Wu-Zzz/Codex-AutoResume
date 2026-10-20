mod managed;
mod protocol;
mod quota;
mod runtime;
pub mod scheduler;
pub mod store;
mod terminal;

use runtime::{AppState, Candidate, Snapshot};
use std::sync::Arc;
use tauri::{
    tray::{MouseButton, MouseButtonState, TrayIconEvent},
    Emitter, Manager, PhysicalPosition, PhysicalRect, PhysicalSize, State,
};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

type Shared = Arc<AppState>;

#[tauri::command]
async fn get_snapshot(state: State<'_, Shared>) -> Result<Snapshot, String> {
    state.combined_snapshot().await
}

#[tauri::command]
async fn list_candidates(state: State<'_, Shared>) -> Result<Vec<Candidate>, String> {
    let mut runtime = state.runtime.lock().await;
    let result = runtime.list_candidates().await;
    state.publish(&mut runtime).await;
    result
}

#[tauri::command]
async fn add_task(state: State<'_, Shared>, thread_id: String) -> Result<(), String> {
    let mut runtime = state.runtime.lock().await;
    let result = runtime.add_task(thread_id).await;
    state.publish(&mut runtime).await;
    result
}

#[tauri::command]
async fn set_task_enabled(
    state: State<'_, Shared>,
    thread_id: String,
    enabled: bool,
) -> Result<(), String> {
    if state.is_managed(&thread_id).await? {
        return state.managed.set_auto_resume(&thread_id, enabled).await;
    }
    let mut runtime = state.runtime.lock().await;
    runtime.set_enabled(&thread_id, enabled)?;
    state.publish(&mut runtime).await;
    Ok(())
}

#[tauri::command]
async fn set_paused(state: State<'_, Shared>, paused: bool) -> Result<(), String> {
    state.managed.pause_all(paused).await?;
    let mut runtime = state.runtime.lock().await;
    runtime.set_paused(paused)?;
    state.publish(&mut runtime).await;
    Ok(())
}

#[tauri::command]
async fn remove_task(state: State<'_, Shared>, thread_id: String) -> Result<(), String> {
    if state.is_managed(&thread_id).await? {
        return state.managed.remove(&thread_id).await;
    }
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
    let mut runtime = state.runtime.lock().await;
    let result = runtime.login_url().await;
    state.publish(&mut runtime).await;
    drop(runtime);
    let url = result?;
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

#[tauri::command]
async fn choose_project(app: tauri::AppHandle) -> Result<Option<String>, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("选择项目文件夹")
        .pick_folder(move |path| {
            let _ = sender.send(path);
        });
    receiver
        .await
        .map_err(|_| "文件夹选择已中断".to_owned())?
        .map(|file| {
            file.into_path()
                .map_err(|_| "请选择本地文件夹".to_owned())?
                .into_os_string()
                .into_string()
                .map_err(|_| "文件夹路径编码不受支持".into())
        })
        .transpose()
}

#[tauri::command]
async fn launch_managed(
    app: tauri::AppHandle,
    state: State<'_, Shared>,
    cwd: String,
    auto_resume: bool,
) -> Result<String, String> {
    let spec = state.managed.start(cwd, auto_resume).await?;
    let directory = app.path().app_data_dir().map_err(|_| "无法读取应用目录")?;
    if let Err(error) = terminal::open(&spec, &directory).await {
        let _ = state.managed.remove(&spec.id).await;
        return Err(error);
    }
    Ok(spec.id)
}

#[tauri::command]
fn set_window_theme(app: tauri::AppHandle, theme: String) -> Result<(), String> {
    let theme = match theme.as_str() {
        "dark" => Some(tauri::Theme::Dark),
        "light" => Some(tauri::Theme::Light),
        "system" => None,
        _ => return Err("未知外观设置".into()),
    };
    for window in app.webview_windows().values() {
        window.set_theme(theme).map_err(|_| "无法更新窗口外观")?;
    }
    Ok(())
}

#[tauri::command]
fn show_main_window(
    app: tauri::AppHandle,
    thread_id: Option<String>,
    add_task: Option<bool>,
) -> Result<(), String> {
    show_window(&app).map_err(|_| "无法打开主窗口".to_string())?;
    if let Some(thread_id) = thread_id {
        app.emit_to("main", "select-task", thread_id)
            .map_err(|_| "无法定位所选任务".to_string())?;
    }
    if add_task == Some(true) {
        app.emit_to("main", "add-task", ())
            .map_err(|_| "无法打开添加任务".to_owned())?;
    }
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            let _ = show_window(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            list_candidates,
            add_task,
            set_task_enabled,
            set_paused,
            remove_task,
            reconnect,
            login,
            open_download,
            choose_project,
            launch_managed,
            set_window_theme,
            show_main_window
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
                tauri::menu::MenuItem::with_id(app, "pause", "暂停全部", true, None::<&str>)?;
            let quit = tauri::menu::MenuItem::with_id(
                app,
                "quit",
                "退出（将停止自动续跑）",
                true,
                None::<&str>,
            )?;
            let menu = tauri::menu::Menu::with_items(app, &[&show, &pause, &quit])?;
            let mut tray = tauri::tray::TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("Codex AutoResume · 开发预览")
                .show_menu_on_left_click(false)
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        position,
                        rect,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if show_tray_window(app, position, rect).is_err() {
                            let _ = show_window(app);
                        }
                    }
                })
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        let _ = show_window(app);
                    }
                    "pause" => {
                        let state = app.state::<Shared>().inner().clone();
                        tauri::async_runtime::spawn(async move {
                            if state.managed.pause_all(true).await.is_err() {
                                return;
                            }
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
        .on_window_event(|window, event| match (window.label(), event) {
            ("main" | "tray", tauri::WindowEvent::CloseRequested { api, .. }) => {
                if window.hide().is_ok() {
                    api.prevent_close();
                }
            }
            ("tray", tauri::WindowEvent::Focused(false)) => {
                let _ = window.hide();
            }
            _ => {}
        })
        .build(tauri::generate_context!())
        .expect("无法启动 Codex AutoResume")
        .run(|app, event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                let _ = show_window(app);
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        });
}

fn show_window(app: &tauri::AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("main") {
        window.unminimize()?;
        window.show()?;
        window.set_focus()?;
    }
    if let Some(window) = app.get_webview_window("tray") {
        window.hide()?;
    }
    Ok(())
}

fn show_tray_window(
    app: &tauri::AppHandle,
    cursor: PhysicalPosition<f64>,
    rect: tauri::Rect,
) -> tauri::Result<()> {
    let Some(window) = app.get_webview_window("tray") else {
        return show_window(app);
    };
    let monitor = app
        .monitor_from_point(cursor.x, cursor.y)?
        .or(app.primary_monitor()?);
    if let Some(monitor) = monitor {
        let scale = monitor.scale_factor();
        let work_area = *monitor.work_area();
        let margin = 8.0 * scale;
        // Work in physical coordinates so mixed-DPI and negative-origin monitors
        // use the same coordinate system as the tray event.
        let size = PhysicalSize::new(
            (380.0 * scale).min((f64::from(work_area.size.width) - margin * 2.0).max(1.0)),
            (560.0 * scale).min((f64::from(work_area.size.height) - margin * 2.0).max(1.0)),
        );
        let mut anchor = PhysicalRect {
            position: rect.position.to_physical::<f64>(scale),
            size: rect.size.to_physical::<f64>(scale),
        };
        if anchor.size.width <= 0.0 || anchor.size.height <= 0.0 {
            anchor.position = cursor;
            anchor.size = PhysicalSize::new(0.0, 0.0);
        }
        window.set_size(size)?;
        window.set_position(popup_position(anchor, size, work_area, margin))?;
    } else {
        window.center()?;
    }
    window.show()?;
    window.set_focus()
}

fn popup_position(
    anchor: PhysicalRect<f64, f64>,
    size: PhysicalSize<f64>,
    work_area: PhysicalRect<i32, u32>,
    margin: f64,
) -> PhysicalPosition<i32> {
    let left = f64::from(work_area.position.x) + margin;
    let top = f64::from(work_area.position.y) + margin;
    let right = f64::from(work_area.position.x) + f64::from(work_area.size.width) - margin;
    let bottom = f64::from(work_area.position.y) + f64::from(work_area.size.height) - margin;
    let x = anchor.position.x + (anchor.size.width - size.width) / 2.0;
    let below = anchor.position.y + anchor.size.height + margin;
    let y = if below + size.height <= bottom {
        below
    } else {
        anchor.position.y - size.height - margin
    };
    PhysicalPosition::new(
        x.clamp(left, (right - size.width).max(left)).round() as i32,
        y.clamp(top, (bottom - size.height).max(top)).round() as i32,
    )
}

#[cfg(test)]
mod window_tests {
    use super::*;

    #[test]
    fn tray_popup_fits_below_retina_menu_bar() {
        let position = popup_position(
            PhysicalRect {
                position: PhysicalPosition::new(2000.0, 0.0),
                size: PhysicalSize::new(48.0, 48.0),
            },
            PhysicalSize::new(760.0, 1120.0),
            PhysicalRect {
                position: PhysicalPosition::new(0, 48),
                size: PhysicalSize::new(2880, 1752),
            },
            16.0,
        );
        assert_eq!(position, PhysicalPosition::new(1644, 64));
    }

    #[test]
    fn tray_popup_flips_above_bottom_taskbar_and_stays_on_screen() {
        let position = popup_position(
            PhysicalRect {
                position: PhysicalPosition::new(1880.0, 1040.0),
                size: PhysicalSize::new(40.0, 40.0),
            },
            PhysicalSize::new(380.0, 560.0),
            PhysicalRect {
                position: PhysicalPosition::new(0, 0),
                size: PhysicalSize::new(1920, 1040),
            },
            8.0,
        );
        assert_eq!(position, PhysicalPosition::new(1532, 472));
    }

    #[test]
    fn tray_popup_clamps_to_negative_origin_secondary_monitor() {
        let position = popup_position(
            PhysicalRect {
                position: PhysicalPosition::new(-1910.0, -300.0),
                size: PhysicalSize::new(20.0, 20.0),
            },
            PhysicalSize::new(380.0, 560.0),
            PhysicalRect {
                position: PhysicalPosition::new(-1920, -280),
                size: PhysicalSize::new(1920, 1080),
            },
            8.0,
        );
        assert_eq!(position, PhysicalPosition::new(-1912, -272));
    }
}
