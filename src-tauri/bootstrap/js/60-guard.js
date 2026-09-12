// 60-guard —— 4 个函数（拆分自 bootstrap.html，2026-09-11）。
// 共享状态与跨模块调用经 NS（window.__BOOT_NS）。
(function (NS) {
  function stepGuardStart() {
    NS.setStep(4);
    NS.phase('guard');
    NS.status('正在启动守卫…');
    // ⚠ 必须有界（2026-09-11 架构修复）。
    //   旧实现是**裸 invoke** —— 而 Rust 侧 guard_start 内部会调服务管理器
    //   （systemctl/launchctl/schtasks）并做两轮 wait_alive，最长可达约 2 分钟。
    //   一旦任一处挂起，本 Promise 永不 settle → 引导页**永久停在「正在启动守卫…」**，
    //   无重试、无出口。这与「卡在检测环境」是同一根因模式。
    //   现加外层超时兜底，并监听 Rust 侧上报的阶段进度（静默等待与卡死无法区分）。
    return NS.withTimeout(NS.core.invoke('guard_start'), NS.GUARD_START_BUDGET_MS, '守卫启动超时').then(function (r) {
      if (r && r.__timeout) return NS.guardFailed('守卫启动超时：' + (r.error || '服务管理器无响应'));
      if (r && r.__error) return NS.guardFailed('守卫启动异常：' + r.__error);
      if (!r || r.ok !== true) return NS.guardFailed('守卫启动失败：' + ((r && r.error) || '未知'));
      return NS.stepGuardReady(0);
    }).catch(function (e) { return NS.guardFailed('守卫启动异常：' + NS.errText(e)); });
  }

  function stepGuardReady(n) {
    if (n === 0) NS.status('等待守卫就绪…');
    // 同样有界：守卫若在启动中，健康探针可能长时间无响应。
    return NS.withTimeout(NS.core.invoke('guard_ready'), 10000, '守卫就绪探测无响应').then(function (r) {
      if (r && r.__timeout) { if (n >= 40) return NS.guardFailed('守卫未就绪（健康探针持续无响应）'); return NS.wait(500).then(function () { return NS.stepGuardReady(n + 1); }); }
      r = r || {};
      if (r.ready) return NS.stepPanel();
      if (n >= 40) return NS.guardFailed('守卫未就绪（端口 ' + (r.port || '?') + ' 无响应）');
      return NS.wait(500).then(function () { return NS.stepGuardReady(n + 1); });
    }).catch(function () {
      if (n >= 40) return NS.guardFailed('守卫就绪探测失败');
      return NS.wait(500).then(function () { return NS.stepGuardReady(n + 1); });
    });
  }

  function guardFailed(msg) {
    if (NS.coreFrom && NS.coreTo && NS.coreFrom !== NS.coreTo) {
      NS.status('守卫未就绪 · 正在回退内核 v' + NS.coreFrom + '…');
      // 回退也是 npm 安装：同样必须有界，否则回退失败会让引导永久停在此处。
      return NS.withTimeout(NS.core.invoke('core_apply', { version: NS.coreFrom }), NS.CORE_APPLY_BUDGET_MS, '回退安装超时').then(function (r) {
        if (!r || r.ok !== true) { NS.fail(msg + '；回退亦失败：' + ((r && r.error) || '未知')); return null; }
        NS.coreTo = NS.coreFrom; // 已回退：再次失败将直接进入失败态（不再循环回退）
        return NS.withTimeout(NS.core.invoke('guard_start'), NS.GUARD_START_BUDGET_MS, '守卫启动超时').then(function () { return NS.stepGuardReady(0); });
      }).catch(function (e) { NS.fail(msg + '；回退异常：' + NS.errText(e)); return null; });
    }
    NS.fail(msg);
    return null;
  }

  function stepPanel() {
    NS.setStep(5);
    NS.phase('ready');   // 关键：这是「壳已健康启动」的确认信号
    NS.status('环境就绪 · 正在打开控制面板…');
    NS.$('logo').classList.add('done');
    return NS.wait(700).then(function () {
      // 通知 Rust「引导完成 → 健康确认」（内核据此确认桌面版本更新成功 / 清 journal）
      if (NS.core) NS.core.invoke('finish_boot').catch(function () {});
      // 切到壳框架：主帧导航（shell.html 将成为主帧，其 win_ctl / 面板导航均可用）
      setTimeout(NS.gotoShell, 400);
    });
  }

  // ── 导出到 NS（跨模块可调用）──
  NS.stepGuardStart = stepGuardStart;
  NS.stepGuardReady = stepGuardReady;
  NS.guardFailed = guardFailed;
  NS.stepPanel = stepPanel;
})(window.__BOOT_NS);
