export default {
  rules: {
    "no-duplicate-selectors": true,
    // Specificity comparisons across distinct component selectors produce
    // false positives; module boundaries and browser tests guard the cascade.
  },
};
