// 20-env —— 5 个函数（拆分自 bootstrap.html，2026-09-11）。
// 共享状态与跨模块调用经 NS（window.__BOOT_NS）。
(function (NS) {
  function stepEnv() {
    NS.setStep(0);
    NS.phase('env');
    NS.status('正在检测系统环境…');
    var deadline = Date.now() + NS.ENV_PROBE_BUDGET_MS;
    return new Promise(function (resolve) {
      var settled = false;
      function poll() {
        if (settled) return;
        // ⚠ 必须给**每次**查询包一层超时（2026-09-11 二次修复）。
        //   原实现是裸 `core.invoke(...).then(...)`：一旦该 invoke 永不 settle，
        //   `poll()` 就**再也不会被调度** —— 既不报错、也不推进，
        //   界面永久停在「正在检测系统环境…」。
        //   这正是用户实测到的「卡住且不报错」：不是探测慢，而是轮询自己停摆了。
        //   （轮询循环与单次调用不同：它**必须**有独立于被调方的心跳。）
        NS.withTimeout(NS.core.invoke('node_status'), 15000, '环境查询无响应').then(function (st) {
          if (settled) return;
          if (st && st.__timeout) {
            // 单次查询无响应：不就此放弃，继续轮询到总预算耗尽再给出口。
            if (Date.now() >= deadline) { settled = true; NS.failEnvTimeout(NS.lastEnv || {}); resolve(null); return; }
            NS.status('正在检测系统环境…（查询无响应，重试中）');
            setTimeout(poll, 400);
            return;
          }
          st = st || {};
          NS.lastEnv = st;
          // Rust 侧硬上限触发的**明确失败**：立即给出可操作结论，
          // 不等前端预算耗尽（那只会得到一句没有信息量的「超时」）。
          if (st.probeError) {
            settled = true;
            NS.envStuck = st.stuck || null;
            NS.fail('环境探测失败：' + st.probeError);
            NS.$('btnForceNode').style.display = '';
            resolve(null);
            return;
          }
          if (st.probing) {
            var s = st.stuck;
            if (s && s.on) {
              NS.envStuck = s;
              NS.status('正在检测系统环境…（' + s.on + ' · 已 ' + Math.round((s.ms || 0) / 1000) + 's 无响应）');
            }
            if (Date.now() >= deadline) {
              settled = true;
              NS.failEnvTimeout(st);
              resolve(null);
              return;
            }
            setTimeout(poll, 400);
            return;
          }
          settled = true;
          var p = NS.afterEnv(st);
          if (p && p.then) { p.then(function () { resolve(null); }, function () { resolve(null); }); } else { resolve(null); }
        }).catch(function (e) {
          if (settled) return;
          settled = true;
          NS.fail('环境检测异常：' + NS.errText(e));
          resolve(null);
        });
      }
      poll();
    });
  }

  function failEnvTimeout(st) {
    NS.envStuck = (st && st.stuck) || null;
    var extra = (NS.envStuck && NS.envStuck.on)
      ? ' · 卡在：' + NS.envStuck.on + '（已 ' + Math.round((NS.envStuck.ms || 0) / 1000) + 's 无响应）'
      : '';
    NS.fail('环境检测超时（探针无响应，可能有异常的可执行文件占位）' + extra);
    // 给出「跳过检测直接安装」出口：这是**唯一**能让用户自救的路径
    // （下载 Node 不需要本机已有 Node）。
    NS.$('btnForceNode').style.display = '';
  }

  function afterEnv(st) {
    if (st.busy) { NS.setStep(1); NS.status(st.status || '正在准备 Node.js 运行环境…'); return NS.stepNodeWait(); }
    if (!st.installed) {
      NS.setStep(1);
      return NS.probeMirrorThen(function () {
        NS.status('未检测到 Node.js · 正在补全运行环境…');
        return NS.core.invoke('start_node_install').then(function () { return NS.stepNodeWait(); });
      });
    }
    // ⚠ 必须校验**最低门槛**：后端一直回传 minOk（DSH 要求 Node >= v22.12），
    //   而前端曾长期忽略它 —— 装了旧版 Node 也照常放行，直到内核启动才失败。
    if (st.minOk === false) {
      NS.setStep(1);
      return NS.probeMirrorThen(function () {
        NS.status('Node.js ' + st.installed + ' 低于最低要求（' + (st.minRequired || 'v22.12') + '）· 正在升级…');
        return NS.core.invoke('start_node_install').then(function () { return NS.stepNodeWait(); });
      });
    }
    NS.nodeVer = st.installed;
    return NS.stepNodeDone();
  }

  function stepNodeWait() {
    NS.phase('node');
    return new Promise(function (resolve) {
      var done = false;
      // 同样包超时：等待安装完成期间也不能因单次查询无响应而永久静默。
      var t = setInterval(function () {
        NS.withTimeout(NS.core.invoke('node_status'), 15000, '环境查询无响应').then(function (st) {
          st = st || {};
          if (st.__timeout) return;   // 下次 tick 重试
          if (!st.busy && st.installed) { if (!done) { done = true; clearInterval(t); NS.nodeVer = st.installed; resolve(NS.stepNodeDone()); } }
        }).catch(function () {});
      }, 700);
      // 兜底：Node 安装可能长达数分钟；超时不当作成功（交给后续步骤如实报错）
      setTimeout(function () { if (!done) { done = true; clearInterval(t); resolve(NS.stepNodeDone()); } }, 600000);
    });
  }

  function stepNodeDone() {
    NS.setStep(1);
    NS.status('Node.js ' + (NS.nodeVer || '') + ' 已就绪');
    // 环境就绪后**才**进入桌面版本（网络步骤，带超时与跳过出口）
    return NS.wait(350).then(NS.stepShellUpdate);
  }

  // ── 导出到 NS（跨模块可调用）──
  NS.stepEnv = stepEnv;
  NS.failEnvTimeout = failEnvTimeout;
  NS.afterEnv = afterEnv;
  NS.stepNodeWait = stepNodeWait;
  NS.stepNodeDone = stepNodeDone;
})(window.__BOOT_NS);
