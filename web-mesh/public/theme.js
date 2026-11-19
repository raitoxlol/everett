try {
  const saved = JSON.parse(localStorage.getItem('everett.mesh.display.v1') || '{}');
  const dark = saved?.theme === 'dark' ||
    (saved?.theme === 'system' && matchMedia('(prefers-color-scheme: dark)').matches);
  document.documentElement.dataset.theme = dark ? 'dark' : 'light';
} catch {
  // Keep the document default if this browser cannot read the saved preference.
}
