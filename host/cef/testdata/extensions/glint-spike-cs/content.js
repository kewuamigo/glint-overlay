(function () {
  try {
    window.__glintExtSpike = true;
    document.documentElement.setAttribute("data-glint-ext-spike", "1");
  } catch (e) {
    /* ignore */
  }
})();
