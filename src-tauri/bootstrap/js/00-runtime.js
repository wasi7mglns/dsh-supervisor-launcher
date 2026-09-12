// 00-runtime —— NS 命名空间、共享状态、全局错误处理与窗口栏（拆分自 bootstrap.html，2026-09-11）。
//
// 拆分原因：原 802 行单块脚本，一处语法错会导致**全页不执行**（已真实发生）。
// 现按职责分为 9 个文件，每个文件独立语法检查（门禁 G5）。
//
// ⚠ 本文件必须**最先加载**：全局 onerror / unhandledrejection 若不先注册，
//   后续文件里的错误就无人捕获（不变量 F2 失效）。
window.__BOOT_NS = window.__BOOT_NS || {};
(function (NS) {
  NS.core = (window.__TAURI__ && window.__TAURI__.core) || null;
  NS.evt = (window.__TAURI__ && window.__TAURI__.event) || null;
  NS.$ = function (id) { return document.getElementById(id); };
  NS.fatalShown = false;
  NS.cur = -1;
  NS.skipAction = null;
  NS.nodeVer = null;
  NS.lastEnv = null;
  NS.envStuck = null;
  NS.warmMirror = null;
  NS.warmTimer = null;
  NS.lastMirror = null;
  NS.shellId = null;
  NS.updPlan = null;
  NS.lastError = null;
  NS.coreFrom = null;
  NS.coreTo = null;
  NS.lastPlan = null;
  NS.stepNames = ['st-env', 'st-node', 'st-shell', 'st-core', 'st-guard', 'st-panel'];
  NS.SHELL_CHECK_BUDGET_MS = 45000;
  NS.SHELL_DOWNLOAD_BUDGET_MS = 300000;
  NS.ENV_PROBE_BUDGET_MS = 45000;
  NS.CORE_PLAN_BUDGET_MS = 90000;
  NS.GUARD_START_BUDGET_MS = 200000;
  NS.CORE_APPLY_BUDGET_MS = 1020000;

  function showFatal(text) {
    if (NS.fatalShown) return;
    NS.fatalShown = true;
    try {
      var el = document.getElementById("fatal");
      if (el) {
        el.style.display = "block";
        var t = document.getElementById("fatalText");
        if (t) t.textContent = String(text);
      }
      if (NS.core && NS.core.invoke) {
        try { NS.core.invoke("shell_set_phase", { phase: "error" }); } catch (e) {}
      }
    } catch (e) {}
  }
  function gotoShell() {
    try { window.location.replace('shell.html'); } catch (e) {}
  }

  window.addEventListener("error", function (e) {
    var where = (e && e.filename ? e.filename + ":" + e.lineno : "unknown");
    NS.showFatal("界面脚本错误（" + where + "）：" + ((e && e.message) || "unknown"));
  });
  window.addEventListener("unhandledrejection", function (e) {
    var r = e && e.reason;
    NS.showFatal("未处理的异步错误：" + ((r && (r.message || r)) || "unknown"));
  });
  if (!NS.core || !NS.core.invoke) {
    NS.showFatal("Tauri IPC 不可用：本页面必须在主帧中加载（需要 IPC 的页面不能放在 iframe 内）。");
  }
  (function () {
    var wctl = function (a) {
      if (!NS.core) return;
      try { NS.core.invoke('win_ctl', { action: a }).catch(function () {}); } catch (e) {}
    };
    var bind = function (id, action) {
      var el = document.getElementById(id);
      if (el) el.addEventListener('click', function () { wctl(action); });
    };
    bind('btnMin', 'minimize');
    bind('btnMax', 'toggle-maximize');
    bind('btnClose', 'hide');
    var drag = document.getElementById('dragRegion');
    // 显式拖动（Linux WebKit drag-region 常不生效的可靠替代）
    if (drag) drag.addEventListener('mousedown', function () { wctl('drag'); });
  })();
  if (NS.evt) {
    NS.evt.listen('shell:goto-panel', function () { NS.gotoShell(); });
  }

  // ── 导出到 NS（跨模块可调用）──
  NS.showFatal = showFatal;
  NS.gotoShell = gotoShell;
})(window.__BOOT_NS);
