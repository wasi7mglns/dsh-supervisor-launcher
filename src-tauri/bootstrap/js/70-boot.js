// 70-boot —— 1 个函数（拆分自 bootstrap.html，2026-09-11）。
// 共享状态与跨模块调用经 NS（window.__BOOT_NS）。
(function (NS) {
  function boot() {
    // ⚠ 必须**明确报错**，不能静默返回（2026-09-11）：
    //   旧实现 `if (!core) return;` 会让页面停在静态文案「正在检测系统环境…」——
    //   既不报错也不推进，用户与排障者都无从下手（真实事故）。
    //   现在若 IPC 不可用，直接给出结论与指引。
    if (!NS.core) {
      NS.fail('Tauri IPC 不可用（window.__TAURI__ 缺失）。请重装桌面壳，或反馈此诊断。');
      return;
    }
    NS.hideFail();
    NS.hideUpdChoice();
    NS.cur = -1; NS.nodeVer = null; NS.coreFrom = null; NS.coreTo = null; NS.lastError = null; NS.updPlan = null;
    // 诊断状态一并重置：重试后不应残留上一次的追踪（否则诊断会误导排障）。
    // lastMirror 不重置（镜像选择结果与本次重试无关，保留可对比）。
    NS.lastEnv = null; NS.envStuck = null;
    NS.$('btnForceNode').style.display = 'none';
    // 镜像预热**与引导并行**（后台，不阻塞）：任何步骤都可展示当前镜像。
    NS.startMirrorWarmup();
    // 从**环境检测**开始（本地快检查在前）；随后 Node → 桌面版本 → 内核 → 守卫 → 控制面板。
    NS.stepEnv().catch(function (e) { NS.fail('引导异常：' + NS.errText(e)); });
  }

  // ── 导出到 NS（跨模块可调用）──
  NS.boot = boot;
})(window.__BOOT_NS);
