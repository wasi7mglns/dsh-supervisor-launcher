// 40-shell-update —— 5 个函数（拆分自 bootstrap.html，2026-09-11）。
// 共享状态与跨模块调用经 NS（window.__BOOT_NS）。
(function (NS) {
  function hideUpdChoice() { NS.$('updChoice').style.display = 'none'; }
  function showUpdChoice(reason) {
    // 注意：变量名不可用 cur —— 外层 cur 是步骤索引，遮蔽会造成步骤条错乱。
    var curVer = (NS.shellId && NS.shellId.version) || "unknown";
    var target = (NS.updPlan && NS.updPlan.latest) || "unknown";
    var kind = (NS.shellId && NS.shellId.installKind) || "unknown";
    NS.$('updMsg').textContent = reason || "更新未能完成";
    NS.$('updMeta').textContent = "当前 " + curVer + " · 目标 " + target + " · 安装形态 " + kind;
    NS.$('updChoice').style.display = '';
  }

  /// 自更新被抑制时**让用户看得见**（P1-A）。
  ///
  /// 旧实现在「跳过」分支只写一行状态就继续启动 —— 用户无从得知自己已收不到更新，
  /// 而护栏又没有恢复入口，于是「永久失效 + 无人察觉」。
  /// 现：把原因显示出来，并给出「立即重试」按钮（调 shell_reset_update_guard）。
  function showUpdateSuppressed(reason) {
    var box = NS.$('updSuppressed');
    if (!box) return; // 旧模板无此节点时不报错（渐进增强）
    NS.$('updSuppressedMsg').textContent = '桌面自动更新已暂停：' + reason;
    box.style.display = '';
    var btn = NS.$('btnUpdRetrySuppressed');
    if (btn && !btn.__bound) {
      btn.__bound = true;
      btn.addEventListener('click', function () {
        btn.disabled = true;
        NS.$('updSuppressedMsg').textContent = '正在恢复自更新…';
        NS.core.invoke('shell_reset_update_guard').then(function (r) {
          var plan = (r && r.plan) || {};
          NS.updPlan = plan;
          if (plan && plan.available === true) {
            NS.$('updSuppressedMsg').textContent = '已恢复 · 发现新版本 ' + (plan.latest || '');
            box.style.display = 'none';
            NS.hideFail();
            NS.boot(); // 重新走一遍引导（现在护栏已复位）
          } else {
            NS.$('updSuppressedMsg').textContent = '已恢复自更新（当前已是最新版本）';
            setTimeout(function () { box.style.display = 'none'; }, 3000);
          }
        }).catch(function (e) {
          NS.$('updSuppressedMsg').textContent = '恢复失败：' + NS.errText(e);
        }).then(function () { btn.disabled = false; });
      });
    }
  }

  function stepShellUpdate() {
    var idx = 2; // st-shell 在新顺序中的位置
    NS.setStep(idx);
    NS.phase('shell-update');
    NS.status('正在检查桌面版本…');
    NS.showSkip(function () { NS.skipShellUpdate('用户跳过'); });
    // 双保险：JS 侧超时 + Rust 侧超时，确保任何情况都有结论
    return NS.withTimeout(NS.core.invoke('shell_identity'), 8000, '读取壳身份超时').then(function (id) {
      if (id && !id.__timeout && !id.__error) NS.shellId = id;
      return NS.withTimeout(NS.core.invoke('shell_update_check'), NS.SHELL_CHECK_BUDGET_MS, '检查超时（网络不可达？）');
    }).then(function (r) {
      if (r && r.__timeout) {
        NS.hideSkip();
        NS.status('桌面版本检查超时，继续启动');
        return NS.wait(600).then(NS.stepCorePlan);
      }
      if (r && r.__error) {
        NS.hideSkip();
        NS.status('桌面版本检查异常，继续启动');
        return NS.wait(600).then(NS.stepCorePlan);
      }
      NS.updPlan = r || {};
      // 情形 A：跳过（不可自更新 / 已抑制 / 冷却中）→ 放行，并让**抑制可见**（P1-A）。
      //  旧实现在此静默放行：用户无从得知自己已收不到更新，而护栏又无恢复入口。
      if (r && r.skipped) {
        NS.hideSkip();
        NS.status('已跳过桌面版本检查（' + (r.reason || '') + '）');
        // 只在「因失败被抑制」时提示；「安装形态不可自更新」属事实，提示只会制造噪音。
        if (/冷却|连续失败|抑制/.test(String(r.reason || ''))) {
          NS.showUpdateSuppressed(r.reason || '');
        }
        return NS.wait(600).then(NS.stepCorePlan);
      }
      // 情形 B：无更新 → 放行
      if (r && r.available !== true) {
        NS.hideSkip();
        if (r && r.ok === false) {
          NS.status('桌面版本检查失败，继续启动：' + (r.error || '未知'));
          return NS.wait(900).then(NS.stepCorePlan);
        }
        NS.status('桌面壳已是最新（' + ((NS.shellId && NS.shellId.version) || '?') + '）');
        return NS.wait(400).then(NS.stepCorePlan);
      }
      // 情形 C：有更新 → 下载安装（保留跳过出口：下载耗时且可能失败）
      return NS.stepShellApply();
    });
  }

  function skipShellUpdate(why) {
    NS.hideSkip();
    NS.hideProgress();
    NS.status('已跳过桌面版本检查（' + why + '），继续启动');
    try { if (NS.core) NS.core.invoke('shell_set_phase', { phase: 'shell-update-skipped' }); } catch (e) {}
    return NS.wait(500).then(NS.stepCorePlan);
  }

  function stepShellApply() {
    var target = (NS.updPlan && NS.updPlan.latest) || '';
    NS.status('发现桌面新版本 ' + target + ' · 正在下载…');
    NS.showProgress(0);
    // 下载期间持续显示进度；外层超时保证「零进展」时能失败放行而非永久等待。
    return NS.withTimeout(
      NS.core.invoke('shell_update_apply'),
      NS.SHELL_DOWNLOAD_BUDGET_MS,
      '下载长时间无进展',
    ).then(function (r) {
      NS.hideProgress();
      r = r || {};
      if (r.__timeout) {
        NS.showUpdChoice(r.error || '下载超时');
        return null;
      }
      if (r.__error) {
        NS.showUpdChoice('桌面版本调用异常：' + r.__error);
        return null;
      }
      if (r.ok !== true) {
        NS.showUpdChoice('桌面版本更新失败：' + (r.error || '未知'));
        return null; // 停在此页，等用户选择（重试 / 继续）
      }
      NS.hideSkip();
      if (r.upToDate) { return NS.stepCorePlan(); }
      NS.status('桌面新版本已安装（' + (r.installed || target) + '）· 正在重启…');
      return NS.wait(800).then(function () {
        // 重启进入新版本；旧进程装、新进程跑。重启后 version == pendingVersion → 视为成功。
        // 注：Windows 上安装器会由插件直接结束本进程，通常不会走到这里。
        return NS.core.invoke('shell_restart');
      });
    });
  }

  // ── 导出到 NS（跨模块可调用）──
  NS.hideUpdChoice = hideUpdChoice;
  NS.showUpdChoice = showUpdChoice;
  NS.showUpdateSuppressed = showUpdateSuppressed;
  NS.stepShellUpdate = stepShellUpdate;
  NS.skipShellUpdate = skipShellUpdate;
  NS.stepShellApply = stepShellApply;
})(window.__BOOT_NS);
