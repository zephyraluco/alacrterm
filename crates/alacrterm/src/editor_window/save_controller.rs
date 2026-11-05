//! Save, conflict resolution and reload orchestration.
use super::*;

impl EditorWindow {
    /// 把第 `ix` 个标签编辑器里那份**快照**写回远端。
    ///
    /// 一条链:抓快照 → 落本地缓存那份(编辑器只活在内存里)→ 交给传输 worker 写远端
    /// (见 [`terminal::SshFs::upload_one`])。路上用户接着改不影响这次写回的内容,写完了也只
    /// 把「已写回」记在**快照那一版**上(见 [`Revisions`])。
    pub(super) fn save_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.save_tab_mode(ix, false, window, cx);
    }

    pub(super) fn save_tab_mode(
        &mut self,
        ix: usize,
        overwrite: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(ix) else {
            return;
        };
        let Body::Text(text) = &mut tab.body else {
            return;
        };
        // 没改过就别写(顺带挡掉「打开就按 Ctrl+S」这种空写);上一次还在路上也别叠第二次。
        if !text.is_dirty() || text.is_saving() {
            return;
        }
        let contents = text.document.format.encode(&text.editor.read(cx).value());
        let expected = text.document.version;
        let new_version = RemoteVersion::of(&contents);
        let total = contents.len() as u64;
        let operation_id = SaveOperationId::new();
        text.document.saving = Some(Saving {
            format: TextFormat::decode(
                std::str::from_utf8(&contents).expect("encoded editor text is UTF-8"),
            )
            .1,
            version: new_version,
            phase: SavePhase::Checking,
            id: operation_id,
            revision: text.document.revisions.current,
            done: 0,
            total,
        });
        text.document.outcome = None;
        let cache = tab.request.local_path.clone();
        let lease = tab.request._lease.clone();
        let preview_id = tab.id;
        let remote_path = tab.request.remote_path.clone();
        let fs = tab.request.fs.clone();
        cx.notify();

        cx.spawn_in(
            window,
            async move |this: WeakEntity<Self>, cx: &mut gpui::AsyncWindowContext| {
                let _lease = lease;
                if !overwrite {
                    let source = fs.clone();
                    let path = remote_path.clone();
                    let checked =
                        cx.background_executor()
                            .spawn(async move {
                                remote_save::remote_matches(source, path, expected).await
                            })
                            .await;
                    match checked {
                        Ok(true) => {}
                        Ok(false) => {
                            let _ = this.update_in(cx, |this, window, cx| {
                                this.save_conflict(preview_id, operation_id, window, cx)
                            });
                            return;
                        }
                        Err(error) => {
                            let _ = this.update_in(cx, |this, window, cx| {
                                this.finish_save(preview_id, operation_id, Err(error), window, cx)
                            });
                            return;
                        }
                    }
                }
                let active = this
                    .update_in(cx, |this, _, cx| {
                        let Some(tab) = this.tabs.iter_mut().find(|tab| tab.id == preview_id)
                        else {
                            return false;
                        };
                        let Body::Text(text) = &mut tab.body else {
                            return false;
                        };
                        if !Saving::matches(text.document.saving.as_ref(), operation_id) {
                            return false;
                        }
                        if let Some(saving) = &mut text.document.saving {
                            saving.phase = SavePhase::Uploading;
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !active {
                    return;
                }
                let cache_path = cache.clone();
                let written = cx
                    .background_executor()
                    .spawn(async move { std::fs::write(&cache_path, contents) })
                    .await;
                if let Err(error) = written {
                    let message = format!("写本地缓存失败:{error}");
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.finish_save(preview_id, operation_id, Err(message), window, cx)
                    });
                    return;
                }
                // A forced app shutdown may have removed the preview while writing the cache.
                if this.update_in(cx, |_, _, _| ()).is_err() {
                    return;
                }
                let mut events = fs.upload_one(cache.clone(), remote_path.clone()).events;
                while let Some(event) = events.next().await {
                    match event {
                        TransferEvent::Progress(progress) => {
                            let (done, total) = (progress.bytes_done, progress.bytes_total);
                            // 视图已经没了也把这次写回读完:半截的远端文件比多等一会儿糟。
                            let _ = this.update_in(cx, |this, window, cx| {
                                this.on_save_progress(
                                    preview_id,
                                    operation_id,
                                    done,
                                    total,
                                    window,
                                    cx,
                                )
                            });
                        }
                        TransferEvent::Done(result) => {
                            let result = result.map_err(|error| error.to_string());
                            let _ = this.update_in(cx, |this, window, cx| {
                                this.finish_save(preview_id, operation_id, result, window, cx)
                            });
                            return;
                        }
                    }
                }
                // 通道在 `Done` 之前就断了:worker 线程没了(照 `sidebar_panel/files.rs` 那条路)。
                let _ = this.update_in(cx, |this, window, cx| {
                    this.finish_save(
                        preview_id,
                        operation_id,
                        Err("远端文件系统线程已退出".to_string()),
                        window,
                        cx,
                    )
                });
            },
        )
        .detach();
    }

    pub(super) fn save_conflict(
        &mut self,
        id: DocumentId,
        operation: SaveOperationId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) else {
            return;
        };
        let Body::Text(text) = &mut tab.body else {
            return;
        };
        if !Saving::matches(text.document.saving.as_ref(), operation) {
            return;
        }
        text.document.saving = None;
        text.document.outcome = Some(Err("远端文件已被修改，未覆盖远端内容".into()));
        tab.close_after_save = false;
        cx.notify();
        if window.has_active_dialog(cx) {
            return;
        }
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let reload = weak.clone();
            let overwrite = weak.clone();
            dialog
                .title("远端文件已修改")
                .child(
                    div().text_sm().child(
                        "远端内容与打开时不同。保留当前编辑，或明确选择重新加载／覆盖远端。",
                    ),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("editor-conflict-cancel")
                                .label("取消")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("editor-conflict-reload")
                                .label("重新加载（丢弃本地修改）")
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let _ = reload
                                        .update(cx, |this, cx| this.reload_tab(id, window, cx));
                                }),
                        )
                        .child(
                            Button::new("editor-conflict-overwrite")
                                .label("覆盖远端")
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let _ = overwrite.update(cx, |this, cx| {
                                        if let Some(ix) =
                                            this.tabs.iter().position(|tab| tab.id == id)
                                        {
                                            this.save_tab_mode(ix, true, window, cx);
                                        }
                                    });
                                }),
                        ),
                )
                .on_cancel(|_, _, _| true)
        });
    }

    pub(super) fn reload_tab(
        &mut self,
        id: DocumentId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) else {
            return;
        };
        if matches!(&tab.body, Body::Text(text) if text.is_saving()) {
            return;
        }
        let request = tab.request.clone();
        tab.body = Body::Loading;
        tab.close_after_save = false;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let _lease = request._lease.clone();
            let mut events = request
                .fs
                .download_one(request.remote_path, request.local_path)
                .events;
            let mut result = Err("远端重新加载中断".to_string());
            while let Some(event) = events.next().await {
                if let TransferEvent::Done(done) = event {
                    result = done.map_err(|error| error.to_string());
                    break;
                }
            }
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(tab) = this.tabs.iter_mut().find(|tab| tab.id == id) else {
                    return;
                };
                match result {
                    Ok(()) => load_text(tab, window, cx),
                    Err(message) => tab.body = Body::Failed(message),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 写回进度(状态栏那行小字)，按标签身份定位。
    pub(super) fn on_save_progress(
        &mut self,
        preview_id: DocumentId,
        operation_id: SaveOperationId,
        done: u64,
        total: u64,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == preview_id) else {
            return;
        };
        let Body::Text(text) = &mut tab.body else {
            return;
        };
        let Some(saving) = text.document.saving.as_mut() else {
            return;
        };
        if !Saving::matches(Some(saving), operation_id) {
            return;
        }
        saving.done = done;
        if total > 0 {
            saving.total = total;
        }
        cx.notify();
    }

    /// 一次写回收尾(成功 / 失败)。`result` 里带的是已经翻成人话的失败原因。
    ///
    /// 按标签身份定位；标签已经关闭时丢弃结果。
    pub(super) fn finish_save(
        &mut self,
        preview_id: DocumentId,
        operation_id: SaveOperationId,
        result: Result<(), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.tabs.iter().position(|tab| tab.id == preview_id) else {
            return;
        };
        let outcome = {
            let tab = &mut self.tabs[ix];
            let Body::Text(text) = &mut tab.body else {
                return;
            };
            if !Saving::matches(text.document.saving.as_ref(), operation_id) {
                return;
            }
            let Some(saving) = text.document.saving.take() else {
                return;
            };
            match result {
                Ok(()) => {
                    text.document.revisions.mark_saved(saving.revision);
                    text.document.version = saving.version;
                    text.document.format = saving.format;
                    text.document.outcome = Some(Ok(()));
                    Ok(saving.total)
                }
                Err(message) => {
                    text.document.outcome = Some(Err(message.clone()));
                    Err(message)
                }
            }
        };
        match outcome {
            Ok(size) => {
                // 远端那份变了:状态栏上的大小跟着变,文件管理器那边把目录重读一次。
                self.tabs[ix].request.size = size;
                refresh_parent_dir(&self.tabs[ix], cx);
                if self.tabs[ix].close_after_save
                    && !matches!(&self.tabs[ix].body, Body::Text(text) if text.is_dirty())
                {
                    // 关这个标签(或关窗口那一路的某一个)那一下点的「保存」:写回成功了才真的收。
                    // ⚠️ 关的是**最后一个**标签时 [`Self::close_tab`] 顺手把窗口也收掉(见
                    // [`Self::save_then_close`])。
                    self.close_tab(ix, window, cx);
                } else {
                    self.tabs[ix].close_after_save = false;
                }
            }
            Err(message) => {
                // 这次没成 ⇒ 别再自动关,让用户看着错误自己决定。
                self.tabs[ix].close_after_save = false;
                let note = format!("写回远端失败:{message}");
                log::warn!("{}:{note}", self.tabs[ix].request.remote_path);
                window.push_notification(Notification::error(note), cx);
            }
        }
        cx.notify();
    }
}
