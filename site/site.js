// Everett site: copy-to-clipboard + subtle scroll reveal (skipped under
// prefers-reduced-motion).
(function () {
  'use strict';

  document.querySelectorAll('.copy').forEach(function (btn) {
    btn.addEventListener('click', function () {
      var el = document.getElementById(btn.getAttribute('data-copy'));
      if (!el) return;
      var text = el.textContent;
      var done = function () {
        btn.textContent = 'copied';
        setTimeout(function () { btn.textContent = 'copy'; }, 1600);
      };
      if (navigator.clipboard && navigator.clipboard.writeText) {
        navigator.clipboard.writeText(text).then(done, function () {
          window.prompt('Copy:', text);
        });
      } else {
        window.prompt('Copy:', text);
      }
    });
  });

  if (window.matchMedia('(prefers-reduced-motion: reduce)').matches ||
      !('IntersectionObserver' in window)) {
    return;
  }
  var targets = document.querySelectorAll(
    'section > .wrap > *, .two-col .col, .flow li, .core-grid > div'
  );
  targets.forEach(function (el) { el.classList.add('reveal'); });
  var io = new IntersectionObserver(function (entries) {
    entries.forEach(function (e) {
      if (e.isIntersecting) {
        e.target.classList.add('in');
        io.unobserve(e.target);
      }
    });
  }, { threshold: 0.12 });
  targets.forEach(function (el) { io.observe(el); });
})();
