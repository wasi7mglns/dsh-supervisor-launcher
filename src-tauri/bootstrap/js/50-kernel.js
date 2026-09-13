// 50-kernel —— 3 个函数（拆分自 bootstrap.html，2026-09-11）。
// 共享状态与跨模块调用经 NS（window.__BOOT_NS）。
(function (NS) {
  function stepCorePlan() {
    NS.setStep(3);
    NS.phase('kernel');
    NS.status('正在检查内核版本…');
    return NS.withTimeout(NS.core.invoke('core_plan'), NS.CORE_PLAN_BUDGET_MS, '内核版本检查超时（网络不可达？）').then(function (p) {
      if (p && p.__timeout) { NS.fail('内核版本检查超时（网络不可达？）'); return null; }
      if (p && p.__error) { NS.fail('内核版本检查异常：' + p.__error); return null; }
      NS.lastPlan = p || {};
      NS.coreFrom = p.installed || null;
      if (!p.installed) {
        NS.status('未安装内核 · 正在安装标准产品包…' + (NS.mirrorText() ? '（源 ' + NS.mirrorText() + '）' : ''));
        return NS.coreApply(null);
      }
      if (p.action === 'upgrade' && p.latest) {
        NS.status('发现新内核 v' + p.latest + '（当前 v' + p.installed + '）· 正在强制更新…');
        return NS.coreApply(p.latest);
      }
      // ⚠ 2026-09-13 修复（失效模式 f）：**远端版本查询失败时必须如实告知**。
      //   缺陷：core.rs 的 build_plan 在 latest 查询失败时输出 action=unknown 且**带 error 原因**，
      //     而前端在 installed 非空时**不看 p.error**，直接走下面的「内核已是最新」分支。
      //   后果：离线 / 全部镜像不可达时，用户被告知「内核已是最新」，掩盖了
      //     「这次根本没查成」——与仓库「如实回传成败、绝不吞错」的纪律相悖，
      //     也让用户失去唯一的网络诊断线索。
      if (p.error) {
        var mt2 = p.registry ? String(p.registry).replace(/^https?:\/\//, '') : NS.mirrorText();
        NS.status('内核版本检查失败（沿用当前 v' + p.installed + '）：' + p.error + (mt2 ? ' · 源 ' + mt2 : ''));
        NS.coreTo = p.installed;
        return NS.wait(300).then(NS.stepCoreDone);
      }
      // ⚠ 显示**实际命中的镜像**（2026-09-11 修复）：
      //   core_plan 早就回传了 registry 字段，但前端从未使用 —— 数据链路断了。
      var mt = p.registry ? String(p.registry).replace(/^https?:\/\//, '') : NS.mirrorText();
      NS.status('内核已是最新（v' + p.installed + '）' + (mt ? ' · 源 ' + mt : ''));
      NS.coreTo = p.installed;
      return NS.wait(300).then(NS.stepCoreDone);
    }).catch(function (e) { NS.fail('内核版本检查失败：' + NS.errText(e)); });
  }

  function coreApply(version) {
    // 有界：Rust 侧 npm install 上限 15 分钟，此处 17 分钟兜底（含镜像回退与解包）。
    return NS.withTimeout(NS.core.invoke('core_apply', { version: version }), NS.CORE_APPLY_BUDGET_MS, '内核安装超时（已中止等待）').then(function (r) {
      if (r && r.__timeout) { NS.fail('内核安装超时（超过 17 分钟未完成）'); return false; }
      if (r && r.__error) { NS.fail('内核安装异常：' + r.__error); return false; }
      if (!r || r.ok !== true) {
        NS.fail('内核' + (NS.coreFrom ? '更新' : '安装') + '失败：' + ((r && r.error) || '未知错误'));
        return false;
      }
      NS.coreTo = r.version;
      // 校验确实生效：防「装到了别的前缀」（跨平台 npm prefix 不一致的典型症状）
      return NS.core.invoke('core_status').then(function (st) {
        st = st || {};
        if (st.version === r.version) return NS.stepCoreDone();
        NS.fail('内核已安装 v' + r.version + '，但检测到的是 v' + (st.version || '未知') + '（安装前缀不一致）');
        return false;
      });
    }).catch(function (e) { NS.fail('内核安装异常：' + NS.errText(e)); return false; });
  }

  function stepCoreDone() {
    NS.setStep(3);
    NS.status('内核 v' + (NS.coreTo || NS.coreFrom || '') + ' 已就绪');
    return NS.wait(300).then(NS.stepGuardStart);
  }

  // ── 导出到 NS（跨模块可调用）──
  NS.stepCorePlan = stepCorePlan;
  NS.coreApply = coreApply;
  NS.stepCoreDone = stepCoreDone;
})(window.__BOOT_NS);
