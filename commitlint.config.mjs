// Commit message rules, checked for every commit of a pull request.
// See the "Changelog and commits" section of CONTRIBUTING.md.
export default {
  extends: ["@commitlint/config-conventional"],
  rules: {
    // Long URLs and trailers are fine in the body and the footer.
    "body-max-line-length": [0],
    "footer-max-line-length": [0],
  },
};
