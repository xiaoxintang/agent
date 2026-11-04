import { cleanup } from "@testing-library/react";
import { afterEach, vi } from "vitest";

// jsdom does not implement browser layout observers or pseudo-element styles.
Object.defineProperty(globalThis, "ResizeObserver", {
  value: class {
    observe = vi.fn();
    unobserve = vi.fn();
    disconnect = vi.fn();
  },
});
const getComputedStyle = window.getComputedStyle.bind(window);
window.getComputedStyle = (element) => getComputedStyle(element);

Object.defineProperty(window, "matchMedia", {
  writable: true,
  value: vi.fn().mockImplementation(() => ({
    matches: false,
    addListener: vi.fn(),
    removeListener: vi.fn(),
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  })),
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});
