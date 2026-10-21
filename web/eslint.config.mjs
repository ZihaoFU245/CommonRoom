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
    rules: {
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
    files: ["src/api/requestAbort.ts"],
    rules: { "no-restricted-properties": "off" },
  },
);
