#!/usr/bin/env python3
"""
Aether metrics hook — runs on Claude Code 'Stop' event.
Spawns a background `claude -p --model haiku` to evaluate the turn.
Writes turn-metrics event into the session JSONL. No API key needed.
"""
import json, sys, os, time, fcntl, subprocess, threading

RECAP_DIR = os.path.expanduser("~/.claude/.aether-recaps")

EVAL_PROMPT_TEMPLATE = """You are scoring one turn of a human-AI coding session. Use the recap for context but judge ONLY the current turn.

CONTEXT (prior turns summary):
{recap}

USER PROMPT:
{prompt}

ASSISTANT RESPONSE:
{response}

Score each metric 0.0 to 1.0:

friction: Did the user correct, reject, or express frustration with the assistant?
  0.0 = user continued naturally or gave new instructions
  0.3 = mild redirection ("not that, try X")
  0.7 = clear rejection ("revert", "wrong", "no")
  1.0 = user is frustrated or assistant ignored prior feedback

hallucination: Did the assistant claim to do something it didn't, reference code/files that don't exist, or state unverified facts?
  0.0 = all claims are grounded in visible evidence (tool calls, file reads, actual output)
  0.5 = vague claims without evidence but plausible
  1.0 = fabricated actions, nonexistent references, or false assertions

confidence: How decisive and clear is the assistant in its approach?
  0.0 = hedging, asking unnecessary clarifications, indecisive
  0.5 = reasonable caution with clear direction
  1.0 = direct, decisive, takes ownership

acceptance: How well did the assistant understand and execute the user's intent?
  0.0 = completely misunderstood or ignored the request
  0.5 = partially addressed but missed key aspects
  1.0 = precisely addressed what the user asked for

performance: Quality of the deliverable (code, explanation, analysis)?
  0.0 = broken, incorrect, or useless output
  0.5 = functional but with issues or unnecessary complexity
  1.0 = clean, correct, well-structured output

Write a recap (max 60 words) summarizing what happened and any notable patterns.

Respond with ONLY valid JSON:
{{"friction":0.0,"hallucination":0.0,"confidence":0.0,"acceptance":0.0,"performance":0.0,"recap":"..."}}"""


def get_recap(session_id):
    path = os.path.join(RECAP_DIR, f"{session_id}.json")
    if os.path.exists(path):
        with open(path) as f:
            data = json.load(f)
            return data.get("recap", ""), data.get("turn_index", 0)
    return "", 0


def save_recap(session_id, recap, turn_index):
    os.makedirs(RECAP_DIR, exist_ok=True)
    path = os.path.join(RECAP_DIR, f"{session_id}.json")
    with open(path, "w") as f:
        json.dump({"recap": recap, "turn_index": turn_index}, f)


def write_metrics(transcript_path, turn_index, metrics):
    event = {
        "type": "turn-metrics",
        "turnIndex": turn_index,
        "friction": metrics.get("friction", 0),
        "hallucination": metrics.get("hallucination", 0),
        "confidence": metrics.get("confidence", 0),
        "acceptance": metrics.get("acceptance", 0),
        "performance": metrics.get("performance", 0),
        "recap": metrics.get("recap", ""),
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S.000Z", time.gmtime()),
    }
    line = json.dumps(event, ensure_ascii=False) + "\n"
    with open(transcript_path, "a") as f:
        fcntl.flock(f, fcntl.LOCK_EX)
        f.write(line)
        fcntl.flock(f, fcntl.LOCK_UN)


def evaluate_turn(session_id, transcript_path, prompt, response):
    """Run evaluation in background using claude CLI with haiku."""
    prev_recap, turn_index = get_recap(session_id)

    eval_prompt = EVAL_PROMPT_TEMPLATE.format(
        recap=prev_recap or "none (first turn)",
        prompt=prompt[:1000],
        response=response[:2000],
    )

    try:
        result = subprocess.run(
            ["claude", "-p", "--model", "haiku", "--no-session-persistence"],
            input=eval_prompt,
            capture_output=True,
            text=True,
            timeout=30,
        )
        output = result.stdout.strip()
    except Exception:
        return

    # Parse JSON from output
    try:
        start = output.find("{")
        end = output.rfind("}") + 1
        if start >= 0 and end > start:
            metrics = json.loads(output[start:end])
        else:
            return
    except (json.JSONDecodeError, ValueError):
        return

    write_metrics(transcript_path, turn_index, metrics)
    save_recap(session_id, metrics.get("recap", ""), turn_index + 1)


def main():
    try:
        raw = sys.stdin.read()
        data = json.loads(raw) if raw.strip() else {}
    except (json.JSONDecodeError, EOFError):
        return

    if data.get("hook_event_name") != "Stop":
        return

    session_id = data.get("session_id", "")
    transcript_path = data.get("transcript_path", "")
    response = data.get("last_assistant_message", "")

    if not session_id or not transcript_path or not response:
        return

    # Get the last user prompt from the transcript
    prompt = ""
    try:
        with open(transcript_path) as f:
            for line in f:
                try:
                    ev = json.loads(line)
                    if ev.get("type") == "user" and ev.get("userType") == "external":
                        content = ev.get("message", {}).get("content")
                        if isinstance(content, str) and not content.strip().startswith("<"):
                            prompt = content
                except (json.JSONDecodeError, ValueError):
                    pass
    except Exception:
        return

    if not prompt:
        return

    # Fork a child process to run evaluation in background
    # so the hook returns immediately without killing the work
    pid = os.fork()
    if pid == 0:
        # Child process — detach and run evaluation
        try:
            os.setsid()
            evaluate_turn(session_id, transcript_path, prompt, response)
        except Exception:
            pass
        os._exit(0)


if __name__ == "__main__":
    main()
