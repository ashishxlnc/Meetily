"use client";

import { Button } from '@/components/ui/button';
import { ButtonGroup } from '@/components/ui/button-group';
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip';
import { Copy, Save, Loader2, Search, FolderOpen } from 'lucide-react';
import Analytics from '@/lib/analytics';

interface SummaryUpdaterButtonGroupProps {
  isSaving: boolean;
  isDirty: boolean;
  onSave: () => Promise<void>;
  onCopy: () => Promise<void>;
  onFind?: () => void;
  onOpenFolder: () => Promise<void>;
  hasSummary: boolean;
}

export function SummaryUpdaterButtonGroup({
  isSaving,
  isDirty,
  onSave,
  onCopy,
  onFind,
  onOpenFolder,
  hasSummary
}: SummaryUpdaterButtonGroupProps) {
  return (
    <ButtonGroup>
      {/* Save button */}
      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            variant="outline"
            size="icon"
            className={`${isDirty ? 'bg-green-200' : ""}`}
            onClick={() => {
              Analytics.trackButtonClick('save_changes', 'meeting_details');
              onSave();
            }}
            disabled={isSaving}
          >
            {isSaving ? <Loader2 className="animate-spin" /> : <Save />}
          </Button>
        </TooltipTrigger>
        <TooltipContent>{isSaving ? "Saving" : "Save Changes"}</TooltipContent>
      </Tooltip>

      {/* Copy button */}
      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            variant="outline"
            size="icon"
            onClick={() => {
              Analytics.trackButtonClick('copy_summary', 'meeting_details');
              onCopy();
            }}
            disabled={!hasSummary}
            className="cursor-pointer"
          >
            <Copy />
          </Button>
        </TooltipTrigger>
        <TooltipContent>Copy Summary</TooltipContent>
      </Tooltip>

      {/* Find button */}
      {/* {onFind && (
        <Button
          variant="outline"
          size="icon"
          title="Find in Summary"
          onClick={() => {
            Analytics.trackButtonClick('find_in_summary', 'meeting_details');
            onFind();
          }}
          disabled={!hasSummary}
          className="cursor-pointer"
        >
          <Search />
          <span className="hidden lg:inline">Find</span>
        </Button>
      )} */}
    </ButtonGroup>
  );
}
