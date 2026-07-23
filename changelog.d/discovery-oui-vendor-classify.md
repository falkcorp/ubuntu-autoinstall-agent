### Added

#### Discovery inbox: OUI vendor lookup + device classification

Discovered devices are now enriched with their MAC manufacturer (from the
embedded IEEE OUI registry) and a derived device class — `machine`, `na`
(phones/watches/tablets/IoT or a randomized private MAC), or `unknown`. Two
signals drive the class: the locally-administered MAC bit (which catches modern
iPhones/Watches/Androids with no vendor data at all) and a keyword classifier
over the resolved vendor. Both vendor and class are computed on read, so the
on-disk `discovered-macs.json` shape is unchanged.

Non-machine (`na`) devices are hidden by default in the Discovery page (with a
"N devices hidden" count) **and** are no longer auto-promoted into the machine
registry by the named-device backfill — so an operator's phones and watches
stop cluttering both the inbox and the Machines list.
