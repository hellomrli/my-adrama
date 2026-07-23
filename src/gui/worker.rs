use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

use crate::pipeline;
use crate::project::{Project, Stage};
use crate::stages;

#[derive(Debug, Clone)]
pub enum Job {
    Parse { dry_run: bool },
    Assets { only: Option<String>, dry_run: bool },
    Storyboard {
        scene: Option<u32>,
        shot: Option<String>,
        dry_run: bool,
    },
    Video {
        shot: Option<String>,
        concat: bool,
        dry_run: bool,
    },
    Redo {
        stage: Stage,
        id: String,
        dry_run: bool,
    },
    Approve { stage: Stage },
    /// Lightweight connectivity probe for settings UI.
    TestEndpoint {
        kind: String,
        base_url: String,
        api_key: String,
        model: String,
    },
}

#[derive(Debug)]
pub enum WorkerMsg {
    Started(String),
    Log(String),
    #[allow(dead_code)]
    Progress { current: u32, total: u32, detail: String },
    Finished { ok: bool, message: String },
}

enum WorkerCmd {
    Run { root: PathBuf, job: Job },
    Cancel,
}

pub struct WorkerHandle {
    cmd_tx: Sender<WorkerCmd>,
    cancel_flag: Arc<AtomicBool>,
    pub rx: Receiver<WorkerMsg>,
}

impl WorkerHandle {
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<WorkerCmd>();
        let (msg_tx, msg_rx) = mpsc::channel::<WorkerMsg>();
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let cancel_worker = cancel_flag.clone();

        thread::Builder::new()
            .name("adrama-worker".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = msg_tx.send(WorkerMsg::Finished {
                            ok: false,
                            message: format!("异步运行时启动失败: {e}"),
                        });
                        return;
                    }
                };

                while let Ok(cmd) = cmd_rx.recv() {
                    match cmd {
                        WorkerCmd::Cancel => {
                            cancel_worker.store(true, Ordering::SeqCst);
                        }
                        WorkerCmd::Run { root, job } => {
                            cancel_worker.store(false, Ordering::SeqCst);
                            let label = job_label(&job);
                            let _ = msg_tx.send(WorkerMsg::Started(label.clone()));
                            let result =
                                rt.block_on(run_job(&root, job, &msg_tx, &cancel_worker));
                            match result {
                                Ok(msg) => {
                                    let _ = msg_tx.send(WorkerMsg::Finished {
                                        ok: true,
                                        message: msg,
                                    });
                                }
                                Err(e) => {
                                    let _ = msg_tx.send(WorkerMsg::Finished {
                                        ok: false,
                                        message: format!("{e:#}"),
                                    });
                                }
                            }
                            cancel_worker.store(false, Ordering::SeqCst);
                        }
                    }
                }
            })
            .expect("spawn worker thread");

        Self {
            cmd_tx,
            cancel_flag,
            rx: msg_rx,
        }
    }

    pub fn submit(&self, root: PathBuf, job: Job) -> Result<(), String> {
        self.cancel_flag.store(false, Ordering::SeqCst);
        self.cmd_tx
            .send(WorkerCmd::Run { root, job })
            .map_err(|_| "后台工作线程已停止".into())
    }

    pub fn cancel(&self) {
        self.cancel_flag.store(true, Ordering::SeqCst);
        let _ = self.cmd_tx.send(WorkerCmd::Cancel);
    }
}

fn job_label(job: &Job) -> String {
    match job {
        Job::Parse { dry_run } => {
            if *dry_run {
                "解析（演练）".into()
            } else {
                "解析".into()
            }
        }
        Job::Assets { only, dry_run } => {
            let mut s = if *dry_run {
                "资产（演练）".to_string()
            } else {
                "资产".to_string()
            };
            if let Some(id) = only {
                s.push_str(&format!(" [{id}]"));
            }
            s
        }
        Job::Storyboard {
            scene,
            shot,
            dry_run,
        } => {
            let mut s = if *dry_run {
                "分镜（演练）".to_string()
            } else {
                "分镜".to_string()
            };
            if let Some(n) = scene {
                s.push_str(&format!(" 场={n}"));
            }
            if let Some(id) = shot {
                s.push_str(&format!(" 镜={id}"));
            }
            s
        }
        Job::Video {
            shot,
            concat,
            dry_run,
        } => {
            let mut s = if *dry_run {
                "视频（演练）".to_string()
            } else {
                "视频".to_string()
            };
            if let Some(id) = shot {
                s.push_str(&format!(" 镜={id}"));
            }
            if *concat {
                s.push_str(" +拼接");
            }
            s
        }
        Job::Redo { stage, id, dry_run } => {
            if *dry_run {
                format!("重生成 {stage}/{id}（演练）")
            } else {
                format!("重生成 {stage}/{id}")
            }
        }
        Job::Approve { stage } => format!("审核通过 {stage}"),
        Job::TestEndpoint { kind, .. } => format!("测试连接 · {kind}"),
    }
}

fn check_cancel(flag: &AtomicBool) -> anyhow::Result<()> {
    if flag.load(Ordering::SeqCst) {
        anyhow::bail!("任务已取消");
    }
    Ok(())
}

async fn run_job(
    root: &std::path::Path,
    job: Job,
    msg_tx: &Sender<WorkerMsg>,
    cancel: &AtomicBool,
) -> anyhow::Result<String> {
    if let Job::TestEndpoint {
        kind,
        base_url,
        api_key,
        model,
    } = job
    {
        return test_endpoint(&kind, &base_url, &api_key, &model, msg_tx).await;
    }

    let _ = msg_tx.send(WorkerMsg::Log(format!("打开项目 {}", root.display())));
    let mut proj = Project::open(root)?;
    check_cancel(cancel)?;

    match job {
        Job::Parse { dry_run } => {
            pipeline::require_stage(&proj, Stage::Parse)?;
            check_cancel(cancel)?;
            stages::parse::run(&mut proj, dry_run).await?;
            Ok(if dry_run {
                "解析演练完成".into()
            } else {
                "解析完成 — 请检查 parsed/breakdown.json 后点击审核通过".into()
            })
        }
        Job::Assets { only, dry_run } => {
            if !dry_run {
                pipeline::require_approved(&proj, Stage::Parse)?;
            }
            check_cancel(cancel)?;
            stages::assets::run(&mut proj, only.as_deref(), dry_run).await?;
            Ok(if dry_run {
                "资产生成演练完成".into()
            } else {
                "资产生成完成 — 请检查图片后审核通过".into()
            })
        }
        Job::Storyboard {
            scene,
            shot,
            dry_run,
        } => {
            if !dry_run {
                pipeline::require_approved(&proj, Stage::Assets)?;
            }
            check_cancel(cancel)?;
            stages::storyboard::run(&mut proj, scene, shot.as_deref(), dry_run).await?;
            Ok(if dry_run {
                "分镜演练完成".into()
            } else {
                "分镜完成 — 请检查画面后审核通过".into()
            })
        }
        Job::Video {
            shot,
            concat,
            dry_run,
        } => {
            if !dry_run {
                pipeline::require_approved(&proj, Stage::Storyboard)?;
            }
            check_cancel(cancel)?;
            stages::video::run(&mut proj, shot.as_deref(), concat, dry_run).await?;
            Ok(if dry_run {
                "视频演练完成".into()
            } else {
                "视频完成 — 请检查片段后审核通过".into()
            })
        }
        Job::Redo { stage, id, dry_run } => {
            check_cancel(cancel)?;
            stages::redo::run(&mut proj, stage, &id, dry_run).await?;
            Ok(format!("重生成 {stage}/{id} 完成"))
        }
        Job::Approve { stage } => {
            pipeline::approve(&mut proj, stage)?;
            Ok(format!("已审核通过：{stage}"))
        }
        Job::TestEndpoint { .. } => unreachable!(),
    }
}

async fn test_endpoint(
    kind: &str,
    base_url: &str,
    api_key: &str,
    model: &str,
    msg_tx: &Sender<WorkerMsg>,
) -> anyhow::Result<String> {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        anyhow::bail!("Base URL 为空");
    }
    if api_key.trim().is_empty() {
        anyhow::bail!("API Key 为空");
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()?;

    let _ = msg_tx.send(WorkerMsg::Log(format!("测试 {kind}: {base}")));

    // Google / Gemini style
    if kind.contains("Google") || kind.contains("Veo") || kind.contains("Gemini") {
        let url = format!("{base}/models?key={api_key}");
        let resp = client.get(&url).send().await?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("连接失败 HTTP {status}: {}", truncate(&text, 200));
        }
        return Ok(format!("Google 端点可用（HTTP {status}）"));
    }

    // OpenAI-compatible: models list
    let url = format!("{base}/models");
    let resp = client
        .get(&url)
        .bearer_auth(api_key.trim())
        .send()
        .await?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if status.is_success() {
        let hint = if !model.is_empty() {
            format!("，当前模型配置：{model}")
        } else {
            String::new()
        };
        return Ok(format!("端点可用（HTTP {status}）{hint}"));
    }

    // Some proxies don't expose /models — try a minimal chat probe only if chat-ish
    if status.as_u16() == 404 {
        let chat_url = format!("{base}/chat/completions");
        let body = serde_json::json!({
            "model": if model.is_empty() { "test" } else { model },
            "messages": [{"role":"user","content":"ping"}],
            "max_tokens": 1
        });
        let resp = client
            .post(&chat_url)
            .bearer_auth(api_key.trim())
            .json(&body)
            .send()
            .await?;
        let st = resp.status();
        let t = resp.text().await.unwrap_or_default();
        // 400/401/429 at least proves reachability + auth path
        if st.is_success() || st.as_u16() == 400 || st.as_u16() == 401 || st.as_u16() == 429 {
            return Ok(format!(
                "端点可达（/models 404，chat 探测 HTTP {st}）"
            ));
        }
        anyhow::bail!("连接失败 HTTP {st}: {}", truncate(&t, 200));
    }

    anyhow::bail!("连接失败 HTTP {status}: {}", truncate(&text, 200));
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{t}…")
    }
}
