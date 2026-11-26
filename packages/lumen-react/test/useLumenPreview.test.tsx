// @vitest-environment jsdom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

import { createLumenPreview, useLumenPreview } from "../src/preview.js";

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

it("re-renders a component that selects one field only when that field changes", async () => {
  const preview = createLumenPreview();
  const renders = { whole: 0, selected: 0 };

  const Whole = () => {
    useLumenPreview(preview);
    renders.whole += 1;
    return null;
  };
  const Selected = () => {
    useLumenPreview(preview, (state) => state.isPlaying);
    renders.selected += 1;
    return null;
  };

  const root = createRoot(document.createElement("div"));
  await act(async () => {
    root.render(
      <>
        <Whole />
        <Selected />
      </>,
    );
  });
  expect(renders).toEqual({ selected: 1, whole: 1 });

  await act(async () => {
    await preview.seek(5);
  });
  expect(preview.getSnapshot().frame).toBe(5);
  expect(renders).toEqual({ selected: 1, whole: 2 });

  await act(async () => root.unmount());
});
