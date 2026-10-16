// CTXRemote website: small effects without any library. Loaded in <head>
// (not deferred) so the "js" class is set before anything paints; the rest
// waits for the page. Without JavaScript every element is simply visible.
document.documentElement.classList.add("js");

(function () {
  "use strict";
  var still = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  /** Counts a number up when it scrolls into view (data-count="1234"). */
  function countUp(el) {
    var target = parseFloat(el.getAttribute("data-count"));
    if (isNaN(target)) return;
    var decimals = parseInt(el.getAttribute("data-decimals") || "0", 10);
    var suffix = el.getAttribute("data-suffix") || "";
    var format = function (v) {
      return v.toLocaleString("de-DE", { minimumFractionDigits: decimals, maximumFractionDigits: decimals }) + suffix;
    };
    if (still) {
      el.textContent = format(target);
      return;
    }
    var start = performance.now();
    var from = parseFloat(el.getAttribute("data-from") || "0");
    var duration = 1400;
    function frame(now) {
      var t = Math.min(1, (now - start) / duration);
      var eased = 1 - Math.pow(1 - t, 4);
      el.textContent = format(from + (target - from) * eased);
      if (t < 1) requestAnimationFrame(frame);
    }
    requestAnimationFrame(frame);
  }
  window.ctxCountUp = countUp;

  function ready() {
    // Staggered groups: each child waits a little longer.
    document.querySelectorAll("[data-stagger]").forEach(function (group) {
      Array.prototype.forEach.call(group.children, function (child, i) {
        child.setAttribute("data-reveal", "");
        child.style.setProperty("--d", i * 70 + "ms");
      });
    });

    var reveal = document.querySelectorAll("[data-reveal], [data-count]");
    if (still || !("IntersectionObserver" in window)) {
      reveal.forEach(function (el) {
        el.classList.add("in");
        if (el.hasAttribute("data-count")) countUp(el);
      });
    } else {
      var seen = new IntersectionObserver(
        function (entries) {
          entries.forEach(function (entry) {
            if (!entry.isIntersecting) return;
            entry.target.classList.add("in");
            if (entry.target.hasAttribute("data-count")) countUp(entry.target);
            seen.unobserve(entry.target);
          });
        },
        { rootMargin: "0px 0px -8% 0px", threshold: 0.12 }
      );
      reveal.forEach(function (el) {
        seen.observe(el);
      });
    }

    // Cards light up where the pointer is.
    if (!still && window.matchMedia("(hover: hover)").matches) {
      document.addEventListener("pointermove", function (e) {
        var card = e.target.closest && e.target.closest(".spot");
        if (!card) return;
        var box = card.getBoundingClientRect();
        card.style.setProperty("--mx", e.clientX - box.left + "px");
        card.style.setProperty("--my", e.clientY - box.top + "px");
      });
    }

    // The header gets a line once the page scrolls.
    var top = document.querySelector(".top");
    if (top) {
      var onScroll = function () {
        top.classList.toggle("scrolled", window.scrollY > 8);
      };
      window.addEventListener("scroll", onScroll, { passive: true });
      onScroll();
    }
  }

  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", ready);
  else ready();
})();
