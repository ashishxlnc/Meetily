"use client";

import { useState, useCallback } from 'react';
import { Button } from '@/components/ui/button';
import { ButtonGroup } from '@/components/ui/button-group';
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip';
import { Copy, FolderOpen, RefreshCw } from 'lucide-react';
import Analytics from '@/lib/analytics';
import { RetranscribeDialog } from './RetranscribeDialog';
import { useConfig } from '@/contexts/ConfigContext';


interface TranscriptButtonGroupProps {
  transcriptCount: number;
  onCopyTranscript: () => void;
  onOpenMeetingFolder: () => Promise<void>;
  meetingId?: string;
  meetingFolderPath?: string | null;
  onRefetchTranscripts?: () => Promise<void>;
}


export function TranscriptButtonGroup({
  transcriptCount,
  onCopyTranscript,
  onOpenMeetingFolder,
  meetingId,
  meetingFolderPath,
  onRefetchTranscripts,
}: TranscriptButtonGroupProps) {
  const { betaFeatures } = useConfig();
  const [showRetranscribeDialog, setShowRetranscribeDialog] = useState(false);

  const handleRetranscribeComplete = useCallback(async () => {
    // Refetch transcripts to show the updated data
    if (onRefetchTranscripts) {
      await onRefetchTranscripts();
    }
  }, [onRefetchTranscripts]);

  return (
    <div className="flex items-center justify-center w-full gap-2">
      <ButtonGroup>
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="outline"
              size="icon"
              onClick={() => {
                Analytics.trackButtonClick('copy_transcript', 'meeting_details');
                onCopyTranscript();
              }}
              disabled={transcriptCount === 0}
            >
              <Copy />
            </Button>
          </TooltipTrigger>
          <TooltipContent>{transcriptCount === 0 ? 'No transcript available' : 'Copy Transcript'}</TooltipContent>
        </Tooltip>

        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              size="icon"
              variant="outline"
              onClick={() => {
                Analytics.trackButtonClick('open_recording_folder', 'meeting_details');
                onOpenMeetingFolder();
              }}
            >
              <FolderOpen size={18} />
            </Button>
          </TooltipTrigger>
          <TooltipContent>Open Recording Folder</TooltipContent>
        </Tooltip>

        {betaFeatures.importAndRetranscribe && meetingId && meetingFolderPath && (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                size="icon"
                variant="outline"
                className="bg-gradient-to-r from-blue-50 to-purple-50 hover:from-blue-100 hover:to-purple-100 border-blue-200"
                onClick={() => {
                  Analytics.trackButtonClick('enhance_transcript', 'meeting_details');
                  setShowRetranscribeDialog(true);
                }}
              >
                <RefreshCw size={18} />
              </Button>
            </TooltipTrigger>
            <TooltipContent>Retranscribe to enhance your recorded audio</TooltipContent>
          </Tooltip>
        )}
      </ButtonGroup>

      {betaFeatures.importAndRetranscribe && meetingId && meetingFolderPath && (
        <RetranscribeDialog
          open={showRetranscribeDialog}
          onOpenChange={setShowRetranscribeDialog}
          meetingId={meetingId}
          meetingFolderPath={meetingFolderPath}
          onComplete={handleRetranscribeComplete}
        />
      )}
    </div>
  );
}
