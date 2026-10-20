// The page an invitation link opens. The fragment never
// reaches the server; the quick helper finds both in its file name.
(function () {
  "use strict";
  // A helper's device: "#<ID>-<token>"; a team's support queue:
  // "#t<queue>-<token>" with the team's name after "/" for this page only.
  var device = location.hash.match(/^#(\d{9})-([a-km-np-z2-9]{20})$/);
  var team = location.hash.match(/^#t([a-km-np-z2-9]{8})-([a-km-np-z2-9]{20})(?:\/(.*))?$/);
  var get = document.getElementById("get");
  if (device) {
    var id = device[1];
    document.getElementById("helper-id").textContent = "ID " + id.replace(/(\d{3})(\d{3})(\d{3})/, "$1 $2 $3");
    document.getElementById("helper").hidden = false;
    get.setAttribute("download", "Remocto-Hilfe-" + id + "-" + device[2] + ".exe");
  } else if (team) {
    var name = "";
    try {
      name = decodeURIComponent(team[3] || "").slice(0, 60);
    } catch (e) {
      name = "";
    }
    document.getElementById("helper-label").textContent = "Ihr Support-Team";
    document.getElementById("helper-id").textContent = name || "Support";
    document.getElementById("helper").hidden = false;
    get.setAttribute("download", "Remocto-Hilfe-t" + team[1] + "-" + team[2] + ".exe");
  } else {
    document.getElementById("broken").hidden = false;
  }
  // The quick helper is a Windows program.
  if (!/Windows/.test(navigator.userAgent) || /Windows Phone/.test(navigator.userAgent)) {
    document.getElementById("other-os").hidden = false;
  }
})();
