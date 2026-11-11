import { Composition, nodes as n, sin, time, vec3 } from "@lumiscia/lumen-sdk";

const canvas = { width: 1280, height: 720 } as const;

// A 2D layer as a plane: its raster is composition-sized, so `width` sets how much of the scene
// it spans and the height follows the 16:9 canvas. A plane's own edge shows if the camera can see
// it, so the hills are wider than the view at their depth.
const layer = (source: ReturnType<typeof n.shape>, width: number, y: number, z: number) =>
  n.image_plane_3d({ source, width, position: [0, y, z] });

// An ellipse wider than the canvas, centred at `centerY`, whose top edge is the ridge. A shape's
// `position` is its top-left corner.
const hill = (color: [number, number, number, number], centerY: number) =>
  n.shape({
    geometry_kind: "ellipse",
    width: 2600,
    height: 900,
    position: [640 - 1300, centerY - 450],
    fill_paint: color,
  });

const sky = n.image_plane_3d({
  source: n.background({
    paint: {
      type: "linear",
      start: [0, 0],
      end: [0, 1],
      stops: [
        [0, [20, 24, 64, 255]],
        [1, [244, 140, 96, 255]],
      ],
    },
  }),
  width: 60,
  position: [0, 0, -12],
});

const title = n.image_plane_3d({
  source: n.text({
    content: "2.5D",
    font_family: "Roboto",
    font_weight: 800,
    font_size: 360,
    color: [255, 255, 255, 255],
    alignment_horizontal: "center",
    position: [0, 140],
    max_width: 1280,
  }),
  width: 5,
  position: [0, 0.2, -2],
});

const dots = n.image_plane_3d({
  source: n.shape({
    geometry_kind: "ellipse",
    width: 90,
    height: 90,
    position: [635, 315],
    fill_paint: [255, 255, 255, 180],
  }),
  width: 3,
  position: [0.3, -0.4, 2.5],
});

// The camera swings sideways and dollies in while looking at the title, so nearer layers cross
// the frame faster than farther ones.
const camera = n.camera_3d({
  position: vec3(sin(time.mul(1.3)).mul(1.6), 0.4, time.mul(-0.35).add(5)),
  look_at: true,
  target: [0, 0.2, -2],
});

export const composition = new Composition({
  renderSettings: canvas,
  timeline: { fps: 30, durationSeconds: 4 },
}).addOutput(
  n.render_3d({
    scene: n.merge_3d({
      objects: [
        sky,
        layer(hill([46, 38, 92, 255], 760), 40, -2.6, -9),
        layer(hill([32, 26, 70, 255], 800), 30, -2, -6),
        title,
        layer(hill([14, 12, 32, 255], 840), 12, -0.5, -0.5),
        dots,
        camera,
      ],
    }),
    tone_mapping: "none",
  }),
);
