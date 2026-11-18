import js from "@eslint/js";
import globals from "globals";
import hooks from "eslint-plugin-react-hooks";
import tseslint from "typescript-eslint";

export default tseslint.config(
  { ignores: ["dist/**", "dist-debug/**", "node_modules/**"] },
  {
    files: ["src/**/*.{ts,tsx}", "vite.config.ts"],
    extends: [...tseslint.configs.recommendedTypeChecked],
    languageOptions: {
      parserOptions: {
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
      },
    },
    plugins: { "react-hooks": hooks },
    rules: {
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "error",
      "no-restricted-properties": [
        "error",
        {
          object: "AbortSignal",
          property: "any",
          message: "Use requestAbort so cancellation also works on Chrome 114.",
        },
        {
          object: "AbortSignal",
          property: "timeout",
          message:
            "Use requestAbort for consistent deadlines and timer cleanup.",
        },
      ],
      "@typescript-eslint/no-non-null-assertion": "error",
      "@typescript-eslint/no-unnecessary-type-assertion": "error",
      "@typescript-eslint/no-floating-promises": [
        "error",
        { ignoreVoid: false },
      ],
      "@typescript-eslint/no-misused-promises": "error",
    },
  },
  {
    files: ["src/**/*.{ts,tsx}"],
    ignores: ["src/features/preferences/inputFocus.ts"],
    rules: {
      "no-restricted-syntax": [
        "error",
        {
          selector:
            'CallExpression[callee.property.name="focus"]:not([callee.object.name="window"])',
          message: "Use focusControl to keep mobile text entry opt-in.",
        },
        {
          selector:
            'JSXOpeningElement:not([name.name="button"]) > JSXAttribute[name.name=/^autofocus$/i]',
          message:
            "Use focusControl for automatic focus on non-button elements.",
        },
      ],
    },
  },
  {
    files: ["tests/**/*.{js,mjs}", "scripts/**/*.{js,mjs}"],
    extends: [js.configs.recommended],
    languageOptions: { globals: { ...globals.browser, ...globals.node } },
    rules: {
      "no-unused-vars": [
        "error",
        {
          argsIgnorePattern: "^_",
          caughtErrorsIgnorePattern: "^_",
          varsIgnorePattern: "^_",
        },
      ],
    },
  },
  {
    files: ["src/api/requestAbort.ts"],
    rules: { "no-restricted-properties": "off" },
  },
);
