# E-waste Counter for Windows

The default build follows the basic counter workflow in the supplied videos. Open `EwasteCounter.exe` to go straight to the counter. Setup and operator sign-in are available through `extended-workflows`. Register data stays in `data\counter.sqlite3` beside the executable. Set `EWASTE_COUNTER_DB` to an absolute path to select a separate register. Use one running instance per database.

## Walkthrough workflow

1. Start a ticket; search/select/create a customer, use **Scan customer** for supported keyboard-input data, or deliberately choose Blank for the applicable 803 flow. Customer editing uses a real Phone field and preserves material lines.
2. Choose the material and enter quantity or gross/tare/net weight. The 803 example shows 4 lb × $0.40 = $1.60, plus a $3.40 minimum adjustment, for $5.00 total. The 801 example is one flat-screen TV at $5.00. Other values are explicitly labeled as placeholders, following the owner's instruction to use the video wherever possible. The 800 example uses $5/each and 802 uses $0.40/lb with a $5 minimum as placeholder unit/pricing choices. Applying the 803 minimum per line is also labeled as a placeholder rule because mixed-load grouping was not established by the video.
3. TV/laptop lines open the observed source-category/details step, with selected customer information populated. The category is shared by TV/laptop lines on the ticket. Residential, Education, Government and Business are category choices, alongside the other categories visible in the legacy form. Optional piece count, contact, reason, notes and pickup/drop-off remain optional. Arbitrary classification text is not a substitute for the category choice.
4. Confirm cash received/change, record the result of the separate card transaction, or select the appropriate business charge-to account. Unknown card outcomes require reconciliation before another tender. Charge-to remains pending billing and has a signature line.
5. Complete once to save the ticket and request its customer receipt. F8 requests the internal copy of that same saved ticket. Both new copies include the saved customer's address and Phone # above the items when available, as approved by the owner. History reopens saved tickets. Keep the handwritten tally and its cash/credit filing workflow.

Manual customer lookup stays available. Select an installed Windows printer to print receipts. Card processing and business billing happen outside the app.

Placeholder notes appear in material selection, entry, saved lines and new receipts. Saved-line and receipt notes use the line's saved pricing rule; existing frozen receipts keep their original text. The placeholder labels do not change rates, totals or stored pricing rules. A final site rate sheet remains a future cutover check, not a prerequisite for this video-based version.

## Scanner input

**Scan customer** explicitly arms a keyboard-input capture window. It supports the named AAMVA ANSI v09-12 text profile and its documented newline normalization. The [AAMVA standard](https://www.aamva.org/getmedia/feab821e-baa5-4524-b865-3eb172b51acd/2026-DLID-Card-Design-Standard.pdf) defines the barcode structure. Clip 1's input window at about 00:23 shows an AAMVA ANSI v09 header, within that version range. The screen does not establish raw separators or the device's transport. Serial/SDK devices and other output profiles remain unconfigured.

Scan or paste into the capture window, then click **Review captured customer** when input has finished. Enter is part of the captured data. Only name/address suggestions reach customer review; even a unique match needs deliberate selection. Selecting an existing customer preserves its saved phone. Creating another record is explicit and starts with an empty Phone field.

Raw barcode text stays hidden in capture memory. Capture and working buffers are cleared on review, cancellation, failure, focus loss or ticket change. Keyboard input stays isolated after an interrupted scan until the operator explicitly returns to manual entry. Unknown, incomplete, ambiguous or oversized data uses manual lookup without changing the load.

The site's actual scanner model, keyboard/serial mode, separators, keyboard layout and output still need verification. The adapter tests use synthetic text and actual egui keyboard/paste events; no real ID or scanner is needed for those tests.

## Receipt printer

Open **Printer...**, choose an installed Windows queue and save the choice. Printer selection starts unconfigured, and saving it does not send a test page. With no queue selected, completion and receipt preview still work. The selected driver's printable area controls wrapping and pagination; the app does not change the Windows default printer.

With a queue configured, Complete requests the customer receipt and F8 requests the internal copy. **View receipt** opens the frozen receipt without sending a printer job. Windows discovery and printing run on workers so a slow driver leaves the counter screen responsive.

New receipt contact fields come from the customer saved with that ticket. Blank and missing fields are omitted. Later customer-directory edits do not change a completed receipt, and existing frozen receipts retain their original text. The address and Phone # block is approved for both copies; the actual counter printer and paper layout still need verification.

Each physical attempt saves its receipt, copy, queue and operator before contacting Windows. **Accepted by Windows** includes the Windows job number; check the printer for paper output. A proven **Not submitted** result can be retried to its saved queue. An uncertain or interrupted attempt is never resent automatically. After checking the paper and Windows queue, **Print another copy after checking** explicitly records a linked new request; the earlier attempt may still produce a copy. This also supports choosing another installed queue when the earlier printer is unavailable. The completed ticket and tender remain unchanged throughout print recovery.

Existing preview/simulation history stays separate from physical submissions. An old `Succeeded` preview is not treated as evidence that Windows received paper output. The physical ledger freezes the submitted text and binds it to the saved ticket, allowing older receipts to survive build variants and later rendering changes.

The Windows adapter uses [GDI StartDoc](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/nf-wingdi-startdocw) and [EndDoc](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/nf-wingdi-enddoc). A controlled Microsoft Print to PDF check exercises the driver path without sending to a physical queue. The actual counter printer, paper size and final copy fields still need operator verification.

## Added workflows are excluded by default

Inventory/custody/shipping, documentary review queues and CSV exports, price approvals, finalized financial adjustments, tariff administration, shift closeout, operator sign-in and operator/audit administration are behind the opt-in Cargo feature `extended-workflows`. Ticket and printer history identifies new basic-build activity as local counter activity.

The observed TV/laptop category/contact form remains in the basic flow. Its presence does not enable the broader source-review or regulatory-reporting subsystem.

Unfinished legacy exchange preview work is archived in `../Archive/Scope-Review/20261116-legacy-preview-wip/`; it is not compiled in either build mode. Schema mapping and a separate four-code historical commodity check remain in `../EwasteReview-20261116/Ewaste-Legacy-Exchange-Mapping.md` as reference material. Historical descriptions and negative price values are not activated as current pricing.

## Source layout

| Path | Contents |
| --- | --- |
| `src/main.rs` | Starts the app, selects the register for the build, and handles backup and restore commands. |
| `src/counter/` | Basic register screen, source details, scanner and Windows printer. |
| `src/shared/` | Ticket, pricing and tender rules, receipt records, and UI code used by both registers. |
| `src/store/` | SQLite, saved records, transaction validation and backup/restore. |
| `src/extended/` | Optional register, sign-in, inventory, finance, intake review and pricing administration. |
| `src/tests/` | Rust tests grouped by counter, shared code, store and extended feature. Their owning modules load them during testing. |
| `scripts/` | Build environment, coverage and release checks. |

## Build and checks

The workspace-local Rust toolchain and installed VS 18 Build Tools are configured by `build.ps1`:

Build and coverage commands share a checked x64 developer environment. Repeated commands in the same PowerShell session reuse the verified compiler, headers and libraries. A missing or inconsistent toolchain stops before Cargo starts.

```powershell
.\build.ps1 test
.\build.ps1 clippy
.\build.ps1 build
.\coverage.ps1
```

On a machine with Microsoft Print to PDF installed, the opt-in driver check also records coverage for real Windows submission to a new workspace PDF:

```powershell
.\coverage.ps1 -PrinterPdf
```

Only an explicit opt-in builds/tests the retained extended implementation:

```powershell
.\build.ps1 test -ExtendedWorkflows
.\build.ps1 clippy -ExtendedWorkflows
.\build.ps1 build -ExtendedWorkflows
```

Both variants use schema v7. Opening a supported earlier database adds the physical receipt ledger and an unconfigured printer selection. The basic build uses the existing tickets and receipts and leaves saved operator records for the extended build. If you operate a register with extended sign-in, select a separate database for it using `EWASTE_COUNTER_DB`. Price-approved drafts stay read-only in the basic build. Completed historical receipts remain viewable and reprintable.

## Recovery

Command-line recovery operates on an existing register database and creates a new file; it never overwrites an existing destination:

```powershell
.\EwasteCounter.exe --backup 'F:\absolute\new-backup.sqlite3'
.\EwasteCounter.exe --restore-copy 'F:\absolute\new-backup.sqlite3' --to 'F:\absolute\new-restored.sqlite3'
```

Restore preserves the source and does not change the running database selection. The basic build verifies core records and extension-envelope preservation; the opt-in extended build additionally validates the extended business ledgers. Close the app before restoring from a standalone backup.

See `../EwasteReview-20261116/Ewaste-Implementation-Status.md` for current verification and the evidence separating observed workflow from added recommendations.
