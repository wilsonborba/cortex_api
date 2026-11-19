#!/usr/bin/env python3
"""Load the 200-site research source directory from Hippo into PostgreSQL."""

import argparse
import json
import os
import re
import sys
import urllib.request
from urllib.parse import urlparse

HIPPO_URL = "http://127.0.0.1:8001/api/v1/memories/a14b72ba59394a6cb5b4883a974cc44c"


def slugify(text: str) -> str:
    slug = re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")
    return slug or "general"


def classify_access_mode(host: str, access_note: str) -> str:
    low = access_note.lower()
    if "403" in low or "skipped" in low or "unavailable" in low:
        return "unavailable"
    if host in ("openalex.org", "www.reddit.com", "reddit.com"):
        return "credentialed_api"
    if "api key" in low or "oauth" in low or "signup" in low:
        return "credentialed_api"
    if "public" in low and "api" in low:
        return "public_api"
    return "public_page"


def fetch_directory(hippo_url: str) -> tuple[list[dict], dict[str, str]]:
    with urllib.request.urlopen(hippo_url, timeout=15) as resp:
        payload = json.loads(resp.read().decode("utf-8"))
    content = payload["data"]["content"]
    subject_name = "General"
    subjects: dict[str, str] = {}
    sources: list[dict] = []
    for line in content.splitlines():
        if line.startswith("## "):
            subject_name = line[3:].strip()
            subjects[slugify(subject_name)] = subject_name
        elif line.startswith("|") and not line.startswith("| #") and not line.startswith("|---"):
            parts = [p.strip() for p in line.strip("|").split("|")]
            if len(parts) >= 6 and parts[0].isdigit():
                match = re.match(r"\[([^\]]+)\]\(([^)]+)\)", parts[1])
                if not match:
                    continue
                name, url = match.group(1).strip(), match.group(2).strip()
                host = urlparse(url).netloc.lower()
                if not host:
                    continue
                subj_slug = slugify(subject_name)
                subjects.setdefault(subj_slug, subject_name)
                sources.append(
                    {
                        "host": host,
                        "name": name,
                        "url": url,
                        "description": parts[2],
                        "regions": parts[3],
                        "languages": parts[4],
                        "access_note": parts[5],
                        "access_mode": classify_access_mode(host, parts[5]),
                        "subject_slug": subj_slug,
                    }
                )
    return sources, subjects


def apply_postgres(db_url: str, sources: list[dict], subjects: dict[str, str]) -> None:
    try:
        import psycopg
    except ImportError:
        import psycopg2 as psycopg  # type: ignore

    with psycopg.connect(db_url) as conn:
        with conn.cursor() as cur:
            for slug, name in subjects.items():
                cur.execute(
                    """
                    INSERT INTO research_subjects(slug, name)
                    VALUES (%s, %s)
                    ON CONFLICT (slug) DO UPDATE SET name = EXCLUDED.name
                    """,
                    (slug, name),
                )
            for src in sources:
                is_enabled = src["access_mode"] != "unavailable"
                cur.execute(
                    """
                    INSERT INTO research_sources(
                        host, name, url, description, languages, regions,
                        access_note, access_mode, is_enabled
                    )
                    VALUES (%s, %s, %s, %s, %s, %s, %s, %s, %s)
                    ON CONFLICT (host) DO UPDATE SET
                        name = EXCLUDED.name,
                        url = EXCLUDED.url,
                        description = EXCLUDED.description,
                        languages = EXCLUDED.languages,
                        regions = EXCLUDED.regions,
                        access_note = EXCLUDED.access_note,
                        access_mode = EXCLUDED.access_mode,
                        is_enabled = EXCLUDED.is_enabled,
                        updated_at = NOW()
                    """,
                    (
                        src["host"],
                        src["name"],
                        src["url"],
                        src["description"],
                        src["languages"],
                        src["regions"],
                        src["access_note"],
                        src["access_mode"],
                        is_enabled,
                    ),
                )
                cur.execute(
                    """
                    INSERT INTO research_source_subjects(host, subject_slug)
                    VALUES (%s, %s)
                    ON CONFLICT (host, subject_slug) DO NOTHING
                    """,
                    (src["host"], src["subject_slug"]),
                )
        conn.commit()


def main() -> int:
    parser = argparse.ArgumentParser(description="Load 200-site research catalog from Hippo.")
    parser.add_argument("--hippo-url", default=HIPPO_URL)
    parser.add_argument("--database-url", default=os.environ.get("CORTEX_DATABASE_URL", ""))
    parser.add_argument("--apply", action="store_true", help="Write parsed records to PostgreSQL")
    args = parser.parse_args()

    sources, subjects = fetch_directory(args.hippo_url)
    print(f"Parsed {len(sources)} sources across {len(subjects)} subjects from Hippo.")
    if not args.apply:
        print("Dry run complete; pass --apply with CORTEX_DATABASE_URL to write to PostgreSQL.")
        return 0
    if not args.database_url:
        print("CORTEX_DATABASE_URL is required with --apply", file=sys.stderr)
        return 2
    apply_postgres(args.database_url, sources, subjects)
    print(f"Upserted {len(sources)} sources and {len(subjects)} subjects.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
