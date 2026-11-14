import clipUrl from "../../assets/clip.mp4?url";
import robotoUrl from "../../../crates/text/assets/roboto/Roboto-Regular.ttf?url";
// The glTF sample fox, CC BY 4.0: its credits are in Fox.README.md beside it.
import foxUrl from "../../../crates/engine/tests/fixtures/Fox.glb?url";
import { CLIP, FOX, SANS } from "./theme.js";

export const media = { [CLIP]: clipUrl, [FOX]: foxUrl };
export const fonts = { [SANS]: robotoUrl };
