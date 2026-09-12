// 80-init —— 按钮绑定、事件监听与启动（拆分自 bootstrap.html，2026-09-11）。
//
// 必须**最后加载**：它依赖前面所有模块已挂到 NS 上。
(function (NS) {
  NS.$('btnUpdRetry').addEventListener('click', function () {
    NS.hideUpdChoice();
    NS.status('正在重试桌面版本更新…');
    NS.stepShellApply().catch(function (e) { NS.showUpdChoice('重试异常：' + NS.errText(e)); });
  });
  NS.$('btnUpdSkip').addEventListener('click', function () {
    NS.hideUpdChoice();
    NS.status('已选择继续使用当前版本');
    try { if (NS.core) NS.core.invoke('shell_set_phase', { phase: 'shell-update-skipped' }); } catch (e) {}
    NS.stepCorePlan().catch(function (e) { NS.fail('引导异常：' + NS.errText(e)); });
  });
  NS.$('btnSkipShell').addEventListener('click', function () {
    if (NS.skipAction) NS.skipAction();
  });
  NS.$('btnMirror').addEventListener('click', function () {
    var shown = NS.$('mirrorBox').style.display !== 'none';
    if (shown) { NS.$('mirrorBox').style.display = 'none'; } else { NS.showMirror(); }
  });
  NS.$('btnMirrorProbe').addEventListener('click', function () { NS.loadMirror(); });
  NS.$('btnMirrorSave').addEventListener('click', function () {
    var split = function (v) {
      return String(v || '').split(/[,\n]/).map(function (x) { return x.trim(); }).filter(Boolean);
    };
    var node = split(NS.$('mirrorNode').value);
    var npm = split(NS.$('mirrorNpm').value);
    if (!node.length && !npm.length) { NS.$('mirrorHint').textContent = '请至少填写一项镜像地址'; return; }
    NS.$('mirrorHint').textContent = '正在保存…';
    var jobs = [];
    if (node.length) jobs.push(NS.core.invoke('mirror_set', { kind: 'node', urls: node }));
    if (npm.length) jobs.push(NS.core.invoke('mirror_set', { kind: 'npm', urls: npm }));
    Promise.all(jobs).then(function () {
      NS.$('mirrorHint').textContent = '已保存，正在重新开始引导…';
      return NS.wait(500);
    }).then(function () {
      NS.hideFail();
      NS.$('mirrorBox').style.display = 'none';
      NS.boot();
    }).catch(function (e) {
      NS.$('mirrorHint').textContent = '保存失败：' + e;
    });
  });
  NS.$('btnRetry').addEventListener('click', function () {
    NS.$('btnForceNode').style.display = 'none';
    NS.boot();
  });
  NS.$('btnForceNode').addEventListener('click', function () {
    NS.hideFail();
    NS.$('btnForceNode').style.display = 'none';
    NS.$('mirrorBox').style.display = 'none';
    NS.setStep(1);
    NS.phase('node');
    NS.status('已跳过环境检测 · 正在准备安装 Node.js…');
    NS.probeMirrorThen(function () {
      return NS.core.invoke('start_node_install').then(function () { return NS.stepNodeWait(); });
    });
  });
  NS.$('btnDiag').addEventListener('click', function () {
    var t = NS.diagText();
    try { if (navigator.clipboard) navigator.clipboard.writeText(t); } catch (e) {}
    NS.status('诊断信息已复制：' + t);
  });
  if (NS.evt) {
    NS.evt.listen('guard_progress', function (e) {
      var p = e.payload || {};
      if (p.status) NS.status(p.status);
    });
  }
  if (NS.evt) {
    NS.evt.listen('shell_update_progress', function (e) {
      var p = e.payload || {};
      if (p.installing) { NS.status('下载完成 · 正在安装…'); NS.showProgress(100); return; }
      var total = p.total || 0;
      var got = p.downloaded || 0;
      if (total > 0) {
        var pct = Math.floor((got / total) * 100);
        NS.showProgress(pct);
        NS.status('正在下载桌面版本 ' + pct + '%（' + Math.round(got / 1048576 * 10) / 10 + ' / ' + Math.round(total / 1048576 * 10) / 10 + ' MB）…');
      } else {
        NS.showProgress(null);
        NS.status('正在下载桌面版本（' + Math.round(got / 1048576 * 10) / 10 + ' MB）…');
      }
    });
  }
  if (NS.evt) {
    NS.evt.listen('env_progress', function (e) {
      var p = e.payload || {};
      if (p.busy) { NS.setStep(1); NS.status(p.status || '正在安装 Node.js…'); }
    });
    NS.evt.listen('env_error', function (e) {
      var p = (e && e.payload) || {};
      NS.fail('运行环境安装失败：' + (p.error || '未知'));
    });
  }
  NS.boot();
})(window.__BOOT_NS);
