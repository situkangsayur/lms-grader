#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Analyzer for lms-grader: one JSON request on stdin, one JSON result on stdout.

ops:
  selftest  -> which optional libraries/models are available
  analyze   -> {text, relevance, material_match, ai_likelihood, sources, notes}

Phase 1 (this file): text from quiz essay answers and submission notes/links, URL/DOI extraction,
relevance via Ollama embeddings (bge-m3). Not yet wired, and reported as such in `notes` rather than
faked: submission FILES (needs an LMS API download endpoint), the meeting's slide text for
material_match, and the ONNX AI-text detector (reuse VAR's model from ai-detector-references/).
"""
import json, math, re, sys, urllib.request

URL_RE = re.compile(r"https?://[^\s)>\]\"']+")
DOI_RE = re.compile(r"\b10\.\d{4,9}/[^\s\"<>]+", re.I)


def embed(ollama_url, model, texts):
    req = urllib.request.Request(f"{ollama_url}/api/embed", data=json.dumps({"model": model, "input": texts}).encode(),
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=120) as r:
        return json.load(r)["embeddings"]


def cosine(a, b):
    dot = sum(x * y for x, y in zip(a, b))
    na, nb = math.sqrt(sum(x * x for x in a)), math.sqrt(sum(y * y for y in b))
    return max(0.0, min(1.0, dot / (na * nb))) if na and nb else 0.0


def text_of(kind, payload):
    parts = []
    if kind == "quiz":
        for a in payload.get("answers", []):
            if (a.get("essay_text") or "").strip():
                parts.append(a["essay_text"].strip())
    else:
        for it in payload.get("items", []):
            if it.get("description"):
                parts.append(it["description"])
            if it.get("url"):
                parts.append(it["url"])
    return "\n\n".join(parts)


def analyze(req):
    kind, payload, notes = req.get("kind", ""), req.get("payload") or {}, []
    text = text_of(kind, payload)
    if kind == "assignment" and any(it.get("kind") == "file" for it in payload.get("items", [])):
        notes.append("berkas kiriman belum dibaca: LMS belum punya endpoint API untuk mengunduh berkas (fase 2)")
    sources = sorted(set(URL_RE.findall(text)) | set(DOI_RE.findall(text)))
    relevance = 0.0
    if text.strip():
        try:
            e = embed(req["ollama_url"], req["embed_model"], [req.get("title", ""), text[:8000]])
            relevance = cosine(e[0], e[1])
        except Exception as ex:  # keep going; the lecturer sees why
            notes.append(f"embedding gagal: {ex}")
    else:
        notes.append("tidak ada teks untuk dinilai (kuis pilihan ganda sudah dinilai otomatis oleh LMS)")
    notes.append("material_match belum dihitung: teks slide per pertemuan belum dimuat (fase 2)")
    notes.append("deteksi AI belum tersambung: akan memakai model ONNX VAR (fase 2); nilai 0 = belum diukur")
    return {"text": text[:20000], "relevance": relevance, "material_match": 0.0, "ai_likelihood": 0.0,
            "sources": sources, "notes": notes}


def selftest():
    have = {}
    for mod in ("pymupdf", "docx", "nbformat", "onnxruntime", "tokenizers"):
        try:
            __import__(mod)
            have[mod] = True
        except Exception:
            have[mod] = False
    return {"python": sys.version.split()[0], "modules": have}


def main():
    req = json.load(sys.stdin)
    op = req.get("op")
    out = selftest() if op == "selftest" else analyze(req) if op == "analyze" else {"error": f"op tidak dikenal: {op}"}
    json.dump(out, sys.stdout, ensure_ascii=False)


if __name__ == "__main__":
    main()
