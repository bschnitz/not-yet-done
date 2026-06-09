#!/usr/bin/env bash
set -euo pipefail

# DB zurücksetzen für sauberen Testlauf
rm -f nyd.db

NYD="./target/debug/not-yet-done-cli"

# Extrahiert die UUID aus einer Erfolgsausgabe wie:
# "✓ Task erstellt: [f5493ef4-...] Beschreibung"
# || true verhindert dass grep Exit-Code 1 das Script abbricht
extract_id() {
    echo "$1" | grep -oP '(?<=\[)[0-9a-f-]{36}(?=\])' || true
}

# Extrahiert die vollständige Tag-ID inkl. Präfix aus einer Ausgabe wie:
# "✓ Globaler Tag erstellt: [global-tag:uuid] name"
extract_tag_id() {
    echo "$1" | grep -oP '(?<=\[)(global-tag|project-tag):[0-9a-f-]{36}(?=\])' || true
}


echo "════════════════════════════════════════"
echo " Schema sync"
echo "════════════════════════════════════════"
$NYD db sync

echo ""
echo "════════════════════════════════════════"
echo " Projekte"
echo "════════════════════════════════════════"
out=$($NYD project add "Arbeit"); echo "$out"
ID_PROJEKT_ARBEIT=$(extract_id "$out")

out=$($NYD project add "Privat" --description "Persönliche Todos"); echo "$out"
ID_PROJEKT_PRIVAT=$(extract_id "$out")

$NYD project list

echo ""
echo "════════════════════════════════════════"
echo " Tags"
echo "════════════════════════════════════════"
out=$($NYD tag add "wichtig" --color "#FF5733"); echo "$out"
ID_TAG_WICHTIG=$(extract_tag_id "$out")

out=$($NYD tag add "wartend"); echo "$out"
ID_TAG_WARTEND=$(extract_tag_id "$out")

out=$($NYD tag add "meeting" --project "Arbeit" --color "#3498DB"); echo "$out"
ID_TAG_MEETING=$(extract_tag_id "$out")

out=$($NYD tag add "einkauf" --project "Privat"); echo "$out"
ID_TAG_EINKAUF=$(extract_tag_id "$out")

echo ""
echo "── tag list (alle) ──"
$NYD tag list

echo ""
echo "── tag list --global ──"
$NYD tag list --global

echo ""
echo "── tag list --project Arbeit ──"
$NYD tag list --project "Arbeit"

echo ""
echo "════════════════════════════════════════"
echo " Tasks"
echo "════════════════════════════════════════"
out=$($NYD task add "Arzt anrufen"); echo "$out"
ID_TASK_ARZT=$(extract_id "$out")

out=$($NYD task add "Quartalsbericht schreiben" --project "Arbeit"); echo "$out"
ID_TASK_QUARTAL=$(extract_id "$out")

out=$($NYD task add "Budget planen" --project "Arbeit" --tag "wichtig"); echo "$out"
ID_TASK_BUDGET=$(extract_id "$out")

out=$($NYD task add "Einleitung schreiben" --parent "$ID_TASK_QUARTAL"); echo "$out"
ID_TASK_EINLEITUNG=$(extract_id "$out")

echo ""
echo "── task list (alle) ──"
$NYD task list

echo ""
echo "── task list --project Arbeit ──"
$NYD task list --project "Arbeit"

echo ""
echo "════════════════════════════════════════"
echo " Tasks bearbeiten"
echo "════════════════════════════════════════"
echo "── Projekt-Tag 'meeting' zu Budget-Task hinzufügen ──"
$NYD task edit "$ID_TASK_BUDGET" --add-tag "meeting"

echo "── Projekt 'Privat' zu Arzt-Task hinzufügen ──"
$NYD task edit "$ID_TASK_ARZT" --add-project "Privat"

echo "── Beschreibung des Arzt-Tasks ändern ──"
$NYD task edit "$ID_TASK_ARZT" --description "Arzt anrufen (dringend)"

echo "── Projekt 'Privat' wieder entfernen ──"
$NYD task edit "$ID_TASK_ARZT" --remove-project "Privat"

echo ""
echo "── task list nach edits ──"
$NYD task list

echo ""
echo "════════════════════════════════════════"
echo " Tag bearbeiten"
echo "════════════════════════════════════════"
$NYD tag edit "$ID_TAG_WARTEND" --color "#95A5A6"
$NYD tag list --global

echo ""
echo "════════════════════════════════════════"
echo " Fehlerfälle"
echo "════════════════════════════════════════"
echo "── Tag 'meeting' ohne Projekt-Kontext (sollte TagNotFound) ──"
$NYD task add "Test" --tag "meeting" || true

echo ""
echo "════════════════════════════════════════"
echo " Soft-Delete"
echo "════════════════════════════════════════"
echo "── Einleitungs-Task löschen ──"
$NYD task delete "$ID_TASK_EINLEITUNG"

echo "── task list (Einleitung sollte weg sein) ──"
$NYD task list

echo "── Projekt Privat mit cascade löschen ──"
$NYD project delete "$ID_PROJEKT_PRIVAT" --cascade

echo "── task list (Tasks von Privat sollten weg sein) ──"
$NYD task list

echo ""
echo "════════════════════════════════════════"
echo " Fertig"
echo "════════════════════════════════════════"
