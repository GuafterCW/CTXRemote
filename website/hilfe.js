// The page an invitation link opens: "#<ID>-<token>". The fragment never
// reaches the server; the quick helper finds both in its file name.
(function () {
  "use strict";
  var match = location.hash.match(/^#(\d{9})-([a-km-np-z2-9]{20})$/);
  var get = document.getElementById("get");
  if (match) {
    var id = match[1];
    document.getElementById("helper-id").textContent = id.replace(/(\d{3})(\d{3})(\d{3})/, "$1 $2 $3");
    document.getElementById("helper").hidden = false;
    get.setAttribute("download", "CTXRemote-Hilfe-" + id + "-" + match[2] + ".exe");
  } else {
    document.getElementById("broken").hidden = false;
  }
  // The quick helper is a Windows program.
  if (!/Windows/.test(navigator.userAgent) || /Windows Phone/.test(navigator.userAgent)) {
    document.getElementById("other-os").hidden = false;
  }
})();
