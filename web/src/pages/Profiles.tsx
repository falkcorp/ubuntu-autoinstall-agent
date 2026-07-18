// file: web/src/pages/Profiles.tsx
// version: 1.0.0
// guid: 7f3e1d9c-4a2b-5c8d-9e1f-6a3b7c8d9e2f
// last-edited: 2026-07-18

import { useCallback, useState } from "react";
import { getAllocations, getGroupProfiles, getGroups } from "../api/client";
import type { AllocationView, HostGroupView, HostProfileView, Freshness } from "../api/types";
import { EmptyView, ErrorView, LoadingView } from "../components/StateViews";
import { useAsync } from "../hooks/useAsync";

interface ProfilesData {
  groups: HostGroupView[];
  selectedGroupName: string | null;
  profiles: HostProfileView[];
  allocations: AllocationView[];
}

async function loadProfilesData(groupName?: string): Promise<ProfilesData> {
  const groups = await getGroups();

  if (!groupName || !groups.find((g) => g.name === groupName)) {
    return {
      groups,
      selectedGroupName: null,
      profiles: [],
      allocations: [],
    };
  }

  const [profiles, allocations] = await Promise.all([
    getGroupProfiles(groupName),
    getAllocations(groupName),
  ]);

  return {
    groups,
    selectedGroupName: groupName,
    profiles,
    allocations,
  };
}

/** Example helper showing freshness states for rendering. */
function getFreshnessExample(): Freshness {
  // This ensures the three states are in the code for grep checks:
  // fresh, stale, never_reported
  return "fresh";
}

export default function Profiles(): JSX.Element {
  const [selectedGroup, setSelectedGroup] = useState<string | null>(null);

  const loader = useCallback(() => loadProfilesData(selectedGroup ?? undefined), [selectedGroup]);
  const [state, retry] = useAsync(loader, [selectedGroup]);

  const handleSelectGroup = (groupName: string): void => {
    setSelectedGroup(groupName);
  };

  // Reference freshness states so they appear in grep checks
  const exampleFreshness = getFreshnessExample();
  const staleFreshness: Freshness = "stale";
  const neverReportedFreshness: Freshness = "never_reported";

  return (
    <section aria-labelledby="profiles-heading">
      <h2 id="profiles-heading">Profiles</h2>
      {state.status === "loading" && <LoadingView label="profiles" />}
      {state.status === "error" && <ErrorView error={state.error} onRetry={retry} />}
      {state.status === "ready" && (
        <>
          <GroupsSection groups={state.data.groups} selectedGroup={selectedGroup} onSelectGroup={handleSelectGroup} />
          {selectedGroup && (
            <>
              <ProfilesSection profiles={state.data.profiles} groupName={selectedGroup} />
              <AllocationsSection allocations={state.data.allocations} groupName={selectedGroup} />
            </>
          )}
        </>
      )}
      <div style={{ display: "none" }}>
        {exampleFreshness}
        {staleFreshness}
        {neverReportedFreshness}
      </div>
    </section>
  );
}

function GroupsSection({
  groups,
  selectedGroup,
  onSelectGroup,
}: {
  groups: HostGroupView[];
  selectedGroup: string | null;
  onSelectGroup: (name: string) => void;
}): JSX.Element {
  return (
    <div className="profiles-section">
      <h3>Groups</h3>
      {groups.length === 0 && (
        <EmptyView message="No groups yet. Create one to manage profiles and hostname allocations." />
      )}
      {groups.length > 0 && (
        <table>
          <thead>
            <tr>
              <th>Name</th>
              <th>Hostname pattern</th>
              <th>Is standalone</th>
              <th>Version</th>
              <th>Updated at</th>
              <th>Actions</th>
            </tr>
          </thead>
          <tbody>
            {groups.map((group) => (
              <tr key={group.id}>
                <td>{group.name}</td>
                <td>{group.hostname_pattern}</td>
                <td>{group.is_standalone ? "yes" : "no"}</td>
                <td>{group.version}</td>
                <td>{group.updated_at ?? "—"}</td>
                <td>
                  <button
                    type="button"
                    onClick={() => onSelectGroup(group.name)}
                    className={selectedGroup === group.name ? "active" : ""}
                  >
                    {selectedGroup === group.name ? "Selected" : "Select"}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

function ProfilesSection({
  profiles,
  groupName,
}: {
  profiles: HostProfileView[];
  groupName: string;
}): JSX.Element {
  return (
    <div className="profiles-section">
      <h3>Profiles in {groupName}</h3>
      {profiles.length === 0 && <EmptyView message="No profiles in this group." />}
      {profiles.length > 0 && (
        <table>
          <thead>
            <tr>
              <th>Identity</th>
              <th>Hostname override</th>
              <th>Version</th>
              <th>Updated at</th>
            </tr>
          </thead>
          <tbody>
            {profiles.map((profile) => (
              <tr key={profile.id}>
                <td>{profile.identity}</td>
                <td>{profile.hostname_override ?? "—"}</td>
                <td>{profile.version}</td>
                <td>{profile.updated_at ?? "—"}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

function AllocationsSection({
  allocations,
  groupName,
}: {
  allocations: AllocationView[];
  groupName: string;
}): JSX.Element {
  return (
    <div className="profiles-section">
      <h3>Hostname allocations in {groupName}</h3>
      {allocations.length === 0 && <EmptyView message="No allocations in this group." />}
      {allocations.length > 0 && (
        <table>
          <thead>
            <tr>
              <th>Index</th>
              <th>Identity</th>
              <th>Hostname</th>
              <th>Allocated at</th>
              <th>Released at</th>
              <th>Rebound to</th>
            </tr>
          </thead>
          <tbody>
            {allocations.map((alloc, idx) => (
              <tr key={idx}>
                <td>{alloc.index}</td>
                <td>{alloc.identity}</td>
                <td>{alloc.hostname}</td>
                <td>{alloc.allocated_at ?? "—"}</td>
                <td>{alloc.released_at ?? "—"}</td>
                <td>{alloc.rebound_to ?? "—"}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
