# Free Image to Video: Rust SDK

Free image to video AI with no daily cap: start with no sign-up, keep going on a free account. Every video comes with sound and no watermark. This is the official Rust SDK for [Free Image to Video](https://freeimagetovideo.ai/?utm_source=crates.io&utm_medium=package): turn a prompt into a clip, or [animate a photo](https://freeimagetovideo.ai/video/animate-photo-ai) with the photo as the first frame, from your own Rust code.

```rust,no_run
use freeimagetovideo::{generate_video, Options};

fn main() -> Result<(), freeimagetovideo::Error> {
    // text to video
    let clip = generate_video("a paper lantern drifting over a night river", &Options::new())?;

    // image to video: the photo becomes the first frame
    let moved = generate_video(
        "the camera slowly orbits the statue",
        &Options::new()
            .image("https://example.com/statue.jpg")
            .set("duration", 6)
            .set("aspect_ratio", "9:16"),
    )?;
    println!("{clip}\n{moved}");
    Ok(())
}
```

Each call submits the run, waits for it and returns the URL of the finished clip. Every clip comes with sound. On the site the same model runs as [image to video AI with sound](https://freeimagetovideo.ai/video/image-to-video-ai-with-sound).

## Install

```bash
cargo add freeimagetovideo
```

Blocking client on `ureq`; no async runtime needed.

## API key and plans

Create a key at [freeimagetovideo.ai/settings/apikeys](https://freeimagetovideo.ai/settings/apikeys), then export it:

```bash
export FREEIMAGETOVIDEO_API_KEY=sk-...
```

API runs need a paid plan: each run draws on the plan's full-speed allowance, then waits in the relaxed queue. See [pricing](https://freeimagetovideo.ai/pricing). To make clips without a key, use the generator on [freeimagetovideo.ai](https://freeimagetovideo.ai/?utm_source=crates.io&utm_medium=package): start with no sign-up, keep going on a free account.

## Options

| option | values |
|---|---|
| `duration` | 4, 5, 6, 8, 10, 15 (seconds) |
| `resolution` | `480p` (default), `768p` |
| `aspect_ratio` | `16:9` (default), `9:16`, `1:1` |

Model: MiniMax H3 (`minimax-h3`).

## Lower level

```rust,no_run
use freeimagetovideo::{Client, Options};

fn main() -> Result<(), freeimagetovideo::Error> {
    let client = Client::from_env();
    let id = client.submit("a lighthouse in a storm, waves crashing", &Options::new().set("duration", 5))?;
    let task = client.status(&id)?;   // task.status: pending | processing | success | failed | canceled
    let left = client.credits()?;     // credits left on the account
    let url = client.wait(&id)?;      // polls every 10 seconds, 30 minutes by default
    println!("{} {left} {url}", task.status);
    Ok(())
}
```

## Errors worth catching

- `Error::Auth`: no key, or the key is wrong or deleted
- `Error::Quota`: not enough credits for this run
- `Error::Plan`: API runs need a paid plan
- `Error::Busy`: too many runs generating at once on this account
- `Error::Rejected`: the content filter refused the prompt, or a cooldown is active; reword it
- `Error::Generation`: the run was accepted but failed; carries `error_message`
- `Error::Timeout`: still generating at the deadline; check it later with `Client::status`
- `Error::Api`: anything else, with the server's message

The service answers most refusals as HTTP 200 with a non-zero `code`, so the SDK reads the envelope, not just the status code.

Full docs: [freeimagetovideo.readthedocs.io](https://freeimagetovideo.readthedocs.io/). MIT licensed.
