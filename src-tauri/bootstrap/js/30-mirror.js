// 30-mirror —— 5 个函数（拆分自 bootstrap.html，2026-09-11）。
// 共享状态与跨模块调用经 NS（window.__BOOT_NS）。
(function (NS) {
  function startMirrorWarmup() {
    try { NS.core.invoke('mirror_warmup').catch(function () {}); } catch (e) {}
    var tries = 0;
    if (NS.warmTimer) clearInterval(NS.warmTimer);
    NS.warmTimer = setInterval(function () {
      tries++;
      if (tries > 40) { clearInterval(NS.warmTimer); NS.warmTimer = null; return; }
      // mirror_cached 是**纯读缓存**（无网络 I/O），可安全高频轮询
      NS.core.invoke('mirror_cached').then(function (m) {
        if (!m || !m.ready) return;
        NS.warmMirror = m;
        if (!NS.lastMirror) NS.lastMirror = { mirror: m.nodeBest, latencyMs: m.nodeLatencyMs, probes: m.npmProbes };
        clearInterval(NS.warmTimer); NS.warmTimer = null;
        var host = String(m.nodeBest || '').replace(/^https?:\/\//, '').split('/')[0];
        if (host) NS.status('镜像已就绪：' + host + '（' + m.nodeLatencyMs + 'ms）');
      }).catch(function () {});
    }, 600);
  }

  function mirrorText() {
    if (NS.warmMirror && NS.warmMirror.npmBest) {
      var h = String(NS.warmMirror.npmBest).replace(/^https?:\/\//, '').split('/')[0];
      return h + '（' + NS.warmMirror.npmLatencyMs + 'ms）';
    }
    if (NS.lastMirror && NS.lastMirror.mirror) {
      return String(NS.lastMirror.mirror).replace(/^https?:\/\//, '').split('/')[0];
    }
    return null;
  }

  function probeMirrorThen(fn) {
    NS.status('正在测速并选择最佳镜像源…');
    return NS.withTimeout(NS.core.invoke('node_latest'), 45000, '镜像测速超时').then(function (m) {
      if (m && !m.__timeout && !m.__error && m.ok) {
        NS.lastMirror = m;
        var host = String(m.mirror || '').replace(/^https?:\/\//, '').split('/')[0];
        NS.status('镜像已选：' + host + '（' + m.latencyMs + 'ms）· 目标 Node ' + m.version);
      } else {
        NS.lastMirror = m || null;
        NS.status('镜像测速未完成 · 将使用内置候选继续');
      }
      return NS.wait(600);
    }).then(fn);
  }

  function showMirror() {
    NS.$('mirrorBox').style.display = '';
    NS.loadMirror();
  }

  function loadMirror() {
    NS.$('mirrorHint').textContent = '正在探测各镜像延迟…';
    return NS.core.invoke('mirror_status').then(function (r) {
      r = r || {};
      NS.$('mirrorNode').value = (r.node || []).join(', ');
      NS.$('mirrorNpm').value = (r.npm || []).join(', ');
      var lines = [];
      (r.nodeProbes || []).forEach(function (p) {
        lines.push('Node ' + p.source.replace(/^https?:\/\//, '') + ' ' + (p.ok ? p.latencyMs + 'ms' : '不可达'));
      });
      var okNode = (r.nodeProbes || []).filter(function (p) { return p.ok; }).length;
      var okNpm = (r.npmProbes || []).filter(function (p) { return p.ok; }).length;
      NS.$('mirrorHint').textContent = lines.join(' · ') + "\n内核镜像可达 " + okNpm + '/' + (r.npmProbes || []).length;
    }).catch(function (e) {
      NS.$('mirrorHint').textContent = '探测失败：' + e;
    });
  }

  // ── 导出到 NS（跨模块可调用）──
  NS.startMirrorWarmup = startMirrorWarmup;
  NS.mirrorText = mirrorText;
  NS.probeMirrorThen = probeMirrorThen;
  NS.showMirror = showMirror;
  NS.loadMirror = loadMirror;
})(window.__BOOT_NS);
