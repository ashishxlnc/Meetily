'use client';

import React, { useEffect, useRef } from 'react';
import { appDataDir } from '@tauri-apps/api/path';
import { emit, listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import { recordingService, MeetingDetectionState } from '@/services/recordingService';
import { useRecordingState, RecordingStatus } from '@/contexts/RecordingStateContext';
import { useTranscripts } from '@/contexts/TranscriptContext';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { useConfig } from '@/contexts/ConfigContext';

type AutomaticRecordingOwner = {
  provider: 'microsoft-teams';
  processId?: number;
};

const AUTO_RECORDING_OWNER_KEY = 'meetily_auto_recording_owner';

function automaticMeetingTitle(): string {
  const timestamp = new Date().toLocaleString(undefined, {
    year: 'numeric',
    month: 'short',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  });
  return `Microsoft Teams — ${timestamp}`;
}

/**
 * Coordinates native meeting detection with the existing recording lifecycle.
 * Ownership is intentionally local to this provider: only a recording started
 * here may be stopped here.
 */
export function RecordingAutomationProvider({ children }: { children: React.ReactNode }) {
  const { status, setStatus } = useRecordingState();
  const { clearTranscripts, setMeetingTitle } = useTranscripts();
  const { setIsMeetingActive } = useSidebar();
  const { selectedDevices } = useConfig();

  const statusRef = useRef(status);
  const ownerRef = useRef<AutomaticRecordingOwner | null>(null);
  const teamsActiveRef = useRef(false);
  const userStoppedRef = useRef(false);
  const automaticStopInProgressRef = useRef(false);
  const transitionInProgressRef = useRef(false);

  useEffect(() => {
    statusRef.current = status;
  }, [status]);

  useEffect(() => {
    const unsubscribers: Array<() => void> = [];
    let cancelled = false;

    const register = (unsubscribe: () => void) => {
      if (cancelled) {
        unsubscribe();
      } else {
        unsubscribers.push(unsubscribe);
      }
    };

    const setAutomaticOwner = (owner: AutomaticRecordingOwner | null) => {
      ownerRef.current = owner;
      if (owner) {
        sessionStorage.setItem(AUTO_RECORDING_OWNER_KEY, JSON.stringify(owner));
      } else {
        sessionStorage.removeItem(AUTO_RECORDING_OWNER_KEY);
      }
    };

    const startAutomaticRecording = async (event: MeetingDetectionState) => {
      if (transitionInProgressRef.current || userStoppedRef.current) return;
      if ([
        RecordingStatus.STARTING,
        RecordingStatus.RECORDING,
        RecordingStatus.STOPPING,
        RecordingStatus.PROCESSING_TRANSCRIPTS,
        RecordingStatus.SAVING,
      ].includes(statusRef.current)) return;
      if (await recordingService.isRecording()) return;

      transitionInProgressRef.current = true;
      setAutomaticOwner({ provider: event.provider, processId: event.processId });
      const title = automaticMeetingTitle();

      try {
        console.info('[RecordingAutomation] Starting automatic Teams recording', event);
        setStatus(RecordingStatus.STARTING, 'Microsoft Teams meeting detected…');
        clearTranscripts();
        setMeetingTitle(title);
        await recordingService.startRecordingWithDevices(
          selectedDevices?.micDevice || null,
          selectedDevices?.systemDevice || null,
          title
        );
        setIsMeetingActive(true);
        toast.success('Teams meeting detected', {
          description: 'Recording started automatically.',
        });
      } catch (error) {
        setAutomaticOwner(null);
        userStoppedRef.current = true; // Avoid repeated prompts during this call.
        setStatus(RecordingStatus.IDLE);
        toast.error('Could not start automatic recording', {
          description: error instanceof Error ? error.message : String(error),
        });
      } finally {
        transitionInProgressRef.current = false;
      }
    };

    const stopAutomaticRecording = async () => {
      if (!ownerRef.current || transitionInProgressRef.current) return;
      if (!(await recordingService.isRecording())) {
        setAutomaticOwner(null);
        return;
      }

      transitionInProgressRef.current = true;
      automaticStopInProgressRef.current = true;
      setStatus(RecordingStatus.STOPPING, 'Teams meeting ended…');

      try {
        console.info('[RecordingAutomation] Stopping automatic Teams recording');
        const dataDir = await appDataDir();
        const timestamp = new Date().toISOString().replace(/[:.]/g, '-');
        await recordingService.stopRecording(`${dataDir}/recording-${timestamp}.wav`);
        setAutomaticOwner(null);
        await emit('recording-stop-complete', true);
        toast.success('Teams meeting ended', {
          description: 'Recording stopped automatically.',
        });
      } catch (error) {
        setStatus(RecordingStatus.ERROR, 'Automatic recording could not be stopped');
        toast.error('Could not stop automatic recording', {
          description: error instanceof Error ? error.message : String(error),
        });
      } finally {
        automaticStopInProgressRef.current = false;
        transitionInProgressRef.current = false;
      }
    };

    const setup = async () => {
      const persistedOwner = sessionStorage.getItem(AUTO_RECORDING_OWNER_KEY);
      if (persistedOwner) {
        try {
          ownerRef.current = JSON.parse(persistedOwner) as AutomaticRecordingOwner;
        } catch {
          sessionStorage.removeItem(AUTO_RECORDING_OWNER_KEY);
        }
      }

      const handleDetection = (detection: MeetingDetectionState) => {
        teamsActiveRef.current = detection.active;

        if (detection.active) {
          void startAutomaticRecording(detection);
        } else {
          userStoppedRef.current = false;
          void stopAutomaticRecording();
        }
      };

      // Register first so a transition cannot occur between the snapshot and
      // listener setup. The snapshot then replays an already-active meeting.
      register(await listen<MeetingDetectionState>('meeting-detection-changed', event => {
        handleDetection(event.payload);
      }));

      register(await recordingService.onRecordingStopped(() => {
        if (
          teamsActiveRef.current
          && ownerRef.current
          && !automaticStopInProgressRef.current
        ) {
          // A person stopped the automatically started session. Do not restart
          // until the native detector observes the end of this Teams meeting.
          setAutomaticOwner(null);
          userStoppedRef.current = true;
        }
      }));

      const currentDetection = await recordingService.getMeetingDetectionState();
      teamsActiveRef.current = currentDetection.active;
      if (currentDetection.active) {
        console.info('[RecordingAutomation] Replaying active Teams detection', currentDetection);
        void startAutomaticRecording(currentDetection);
      }
    };

    void setup().catch(error => {
      console.error('[RecordingAutomation] Failed to initialize listeners:', error);
    });

    return () => {
      cancelled = true;
      unsubscribers.forEach(unsubscribe => unsubscribe());
    };
  }, [clearTranscripts, selectedDevices, setIsMeetingActive, setMeetingTitle, setStatus]);

  return <>{children}</>;
}
