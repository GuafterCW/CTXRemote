// The status page: asks the server (/api/status) every 30 seconds and draws
// the answer. If the server does not answer, that is the news.
(function () {
  "use strict";
  var REFRESH_MS = 30000;
  var page = document.querySelector(".status-page");
  var $ = function (id) {
    return document.getElementById(id);
  };
  var checkedAt = 0;
  var drawn = false;

  var percent = function (share, digits) {
    return (share * 100).toLocaleString("de-DE", { minimumFractionDigits: digits, maximumFractionDigits: digits }) + " %";
  };
  var dayName = function (unix) {
    return new Date(unix * 1000).toLocaleDateString("de-DE", { weekday: "short", day: "numeric", month: "long", year: "numeric", timeZone: "UTC" });
  };

  /** The status a day's uptime stands for: class, word. */
  function level(share) {
    if (share === null || share === undefined) return ["none", "noch keine Daten"];
    if (share >= 0.995) return ["good", "voll verfügbar"];
    if (share >= 0.95) return ["warning", "kurze Ausfälle"];
    if (share >= 0.8) return ["serious", "längere Ausfälle"];
    return ["critical", "großteils ausgefallen"];
  }

  /** Running time as a short headline and the rest for the line below. */
  function duration(seconds) {
    var days = Math.floor(seconds / 86400);
    var hours = Math.floor((seconds % 86400) / 3600);
    var minutes = Math.floor((seconds % 3600) / 60);
    if (days > 0) return [days + (days === 1 ? " Tag" : " Tage"), "und " + hours + " Std."];
    if (hours > 0) return [hours + " Std.", "und " + minutes + " Min."];
    return [Math.max(1, minutes) + " Min.", ""];
  }

  function setNumber(el, value, decimals, suffix) {
    var previous = parseFloat(el.getAttribute("data-count") || "0");
    el.setAttribute("data-from", drawn ? String(previous) : "0");
    el.setAttribute("data-count", String(value));
    el.setAttribute("data-decimals", String(decimals || 0));
    el.setAttribute("data-suffix", suffix || "");
    if (window.ctxCountUp) window.ctxCountUp(el);
    else el.textContent = value + (suffix || "");
  }

  function service(name, ok, word) {
    var row = document.querySelector('[data-service="' + name + '"]');
    if (!row) return;
    row.setAttribute("data-ok", ok ? "yes" : "no");
    row.querySelector(".badge").textContent = word || (ok ? "läuft" : "gestört");
  }

  // One tooltip for all bars, shown on hover and keyboard focus.
  var tip = $("tip");
  function showTip(bar) {
    tip.innerHTML = "";
    var title = document.createElement("strong");
    title.textContent = bar.getAttribute("data-day");
    var line = document.createElement("span");
    line.textContent = bar.getAttribute("data-text");
    tip.append(title, line);
    tip.hidden = false;
    var box = bar.getBoundingClientRect();
    var width = tip.offsetWidth;
    var left = Math.min(window.innerWidth - width - 8, Math.max(8, box.left + box.width / 2 - width / 2));
    tip.style.left = left + window.scrollX + "px";
    tip.style.top = box.top + window.scrollY - tip.offsetHeight - 10 + "px";
  }
  function hideTip() {
    tip.hidden = true;
  }
  ["uptime-bars", "session-bars"].forEach(function (id) {
    var holder = $(id);
    holder.addEventListener("pointerover", function (e) {
      if (e.target.classList.contains("bar")) showTip(e.target);
    });
    holder.addEventListener("pointerleave", hideTip);
    holder.addEventListener("focusin", function (e) {
      if (e.target.classList.contains("bar")) showTip(e.target);
    });
    holder.addEventListener("focusout", hideTip);
    // Arrow keys walk the days; only one bar is in the tab order.
    holder.addEventListener("keydown", function (e) {
      if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
      var bar = e.target.closest(".bar");
      var next = bar && (e.key === "ArrowLeft" ? bar.previousElementSibling : bar.nextElementSibling);
      if (!next) return;
      e.preventDefault();
      bar.tabIndex = -1;
      next.tabIndex = 0;
      next.focus();
    });
  });

  function bar(day, cls, text, height, i, last) {
    var el = document.createElement("span");
    el.className = "bar " + cls;
    el.setAttribute("data-day", dayName(day));
    el.setAttribute("data-text", text);
    el.setAttribute("role", "img");
    el.setAttribute("aria-label", dayName(day) + ": " + text);
    el.tabIndex = last ? 0 : -1;
    el.style.setProperty("--h", height);
    el.style.setProperty("--i", i);
    return el;
  }

  function draw(status) {
    var days = status.days || [];
    // Uptime per day.
    var uptimeBars = $("uptime-bars");
    uptimeBars.replaceChildren.apply(
      uptimeBars,
      days.map(function (d, i) {
        var l = level(d.up);
        var text = d.up === null ? l[1] : percent(d.up, 2) + " verfügbar · " + l[1];
        return bar(d.day, l[0], text, 1, i, i === days.length - 1);
      })
    );
    // Sessions per day: one series, so no legend; the heading names it.
    var max = Math.max.apply(null, days.map(function (d) { return d.sessions; }).concat([1]));
    var sessionBars = $("session-bars");
    sessionBars.replaceChildren.apply(
      sessionBars,
      days.map(function (d, i) {
        var text = d.sessions.toLocaleString("de-DE") + (d.sessions === 1 ? " Sitzung" : " Sitzungen");
        return bar(d.day, d.sessions ? "count" : "count zero", text, d.sessions / max, i, i === days.length - 1);
      })
    );
    $("sessions-max").textContent = max.toLocaleString("de-DE");
    var today = days.length ? days[days.length - 1].sessions : 0;
    $("sessions-figure").textContent = today.toLocaleString("de-DE") + " heute";
    $("uptime-figure").textContent = status.uptime === null || status.uptime === undefined ? "" : percent(status.uptime, 2) + " in 90 Tagen";

    // The table for screen readers and anyone who prefers numbers.
    var rows = days
      .slice()
      .reverse()
      .map(function (d) {
        var tr = document.createElement("tr");
        [dayName(d.day), d.up === null ? "–" : percent(d.up, 2), d.sessions.toLocaleString("de-DE")].forEach(function (v) {
          var td = document.createElement("td");
          td.textContent = v;
          tr.append(td);
        });
        return tr;
      });
    $("days-table").replaceChildren.apply($("days-table"), rows);

    setNumber($("online"), status.online);
    setNumber($("relaying"), status.relaying);
    if (status.uptime === null || status.uptime === undefined) $("uptime").textContent = "–";
    else setNumber($("uptime"), status.uptime * 100, 2, " %");
    var running = duration(status.now - status.started);
    $("since").textContent = running[0];
    $("version").textContent = (running[1] ? running[1] + " · " : "") + "Version " + status.version;
    page.classList.add("drawn");
    drawn = true;
  }

  function setState(state, headline, subline) {
    page.setAttribute("data-state", state);
    $("headline").textContent = headline;
    if (subline) $("subline").textContent = subline;
  }

  function freshness() {
    if (!checkedAt) return;
    var ago = Math.round((Date.now() - checkedAt) / 1000);
    var when = ago < 5 ? "gerade eben" : "vor " + ago + " Sekunden";
    $("subline").textContent = "Zuletzt geprüft " + when + ". Die Seite aktualisiert sich selbst.";
  }

  function check() {
    var api = fetch("/api/status", { cache: "no-store" }).then(function (r) {
      if (!r.ok) throw new Error(r.status);
      return r.json();
    });
    var download = fetch("/download/CTXRemote-Setup.exe", { method: "HEAD", cache: "no-store" })
      .then(function (r) {
        return r.ok;
      })
      .catch(function () {
        return false;
      });
    return Promise.all([api.catch(function () { return null; }), download]).then(function (results) {
      var status = results[0];
      var downloads = results[1];
      checkedAt = Date.now();
      service("web", true);
      service("download", downloads, downloads ? "läuft" : "nicht erreichbar");
      if (!status) {
        service("server", false, "nicht erreichbar");
        setState("down", "Server nicht erreichbar");
        freshness();
        return;
      }
      service("server", true);
      draw(status);
      if (downloads) setState("ok", "Alle Systeme laufen");
      else setState("degraded", "Teilweise eingeschränkt");
      freshness();
    });
  }

  check();
  setInterval(check, REFRESH_MS);
  setInterval(freshness, 1000);
})();
