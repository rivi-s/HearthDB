# Optional pfQuest-HDB Hitch Profiler 1.0.1

This is a diagnostic addon, not a DLL or replacement pfQuest installation.
It updates the older profiler supplied with previous install repositories.
Uses the current tested instrumentation: frame/event/callback/decoder timings,
memory and GC thresholds, startup gates, route/map readiness and visibility.
Works with existing HearthDB DLLs. The exp.3 DLL additionally supports native
phase timings and diagnostic dumps. A memory drop does not identify the addon
that originally allocated the garbage; instrumentation itself has some overhead.

## Install
Replace the old Interface/AddOns/pfQuest-HDB-HitchProfiler folder with the
pfQuest-HDB-HitchProfiler folder in this ZIP. Do not install a second copy under
a renamed folder. Run /reload, or restart when also changing a DLL.

## Capture login
It automatically captures 45 seconds after entering the world. Send the chat
output, including the 'profiler 1.0.1' line if available. With the experimental
DLL it also requests Logs/hdb_delivery_exp3.log at capture end.

## Capture a random hitch or quest completion
After the automatic capture finishes, run /hdbhitch on BEFORE reproducing the
problem. When done, run /hdbhitch off and copy the relevant chat lines.
With exp.3, request an updated native timing log after reproduction using:
/run if HDB_DumpDeliveryDiagnostics then DEFAULT_CHAT_FRAME:AddMessage(tostring(HDB_DumpDeliveryDiagnostics())) end
Then attach Logs/hdb_delivery_exp3.log, if created. The old DLL has no native
dump function; send its chat capture instead.

Compare the old and experimental DLL with the same addons and settings. Fully
exit before replacing a DLL. Include pfQuest-HDB/DLL versions, what happened,
and whether the hitch also occurs with the old DLL. Review logs before sharing.

/hdbhitch status reports capture state. /hdbroute reports routing state.
Disable this diagnostic addon after testing; no SavedVariables are added.

## Saved login capture setting
/hdbhitch login off disables automatic capture after entering the world and
stops an automatic capture already running. The setting is saved per character
and survives /reload and future logins. /hdbhitch login on enables automatic
capture on the next world entry. New installations default to login capture ON.

Manual /hdbhitch on and /hdbhitch off remain independent; disabling automatic
login capture does not stop a manually started capture. /hdbhitch status shows
both current capture state and the saved login setting.
