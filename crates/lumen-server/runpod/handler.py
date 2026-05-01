import json
import subprocess

import runpod


def handler(event):
    process = subprocess.run(
        ["/usr/local/bin/lumen-runpod"],
        input=json.dumps(event),
        capture_output=True,
        check=False,
        text=True,
        timeout=None,
    )

    if process.returncode != 0:
        return {
            "ok": False,
            "error": {
                "code": "runpod_worker_failed",
                "message": process.stderr[-1024:],
                "retryable": True,
            },
        }

    return json.loads(process.stdout)


if __name__ == "__main__":
    runpod.serverless.start({"handler": handler})
