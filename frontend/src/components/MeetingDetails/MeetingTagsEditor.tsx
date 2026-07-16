"use client";
import { useState, useEffect, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { Tag as TagIcon, Plus, X } from 'lucide-react';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import type { MeetingTag } from '@/components/Sidebar/SidebarProvider';

// Self-contained: fetches/owns its own tag state independent of the rest of
// the meeting-details data flow, so it can't destabilize the (already large)
// prop chain through PageContent/SummaryPanel. Calls refetchMeetings() after
// any change so the sidebar's tag filter and per-meeting chips stay in sync.
export function MeetingTagsEditor({ meetingId }: { meetingId: string }) {
  const { refetchMeetings } = useSidebar();
  const [tags, setTags] = useState<MeetingTag[]>([]);
  const [allTags, setAllTags] = useState<MeetingTag[]>([]);
  const [isOpen, setIsOpen] = useState(false);
  const [input, setInput] = useState('');
  const [isLoading, setIsLoading] = useState(true);
  const containerRef = useRef<HTMLDivElement>(null);

  const loadTags = useCallback(async () => {
    if (!meetingId || meetingId === 'intro-call') return;
    try {
      const [meetingTags, all] = await Promise.all([
        invoke('api_get_meeting_tags', { meetingId }) as Promise<MeetingTag[]>,
        invoke('api_list_tags') as Promise<MeetingTag[]>,
      ]);
      setTags(meetingTags);
      setAllTags(all);
    } catch (error) {
      console.error('Failed to load tags:', error);
    } finally {
      setIsLoading(false);
    }
  }, [meetingId]);

  useEffect(() => {
    setIsLoading(true);
    loadTags();
  }, [loadTags]);

  useEffect(() => {
    if (!isOpen) return;
    const handleClickOutside = (e: MouseEvent) => {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        setIsOpen(false);
      }
    };
    document.addEventListener('mousedown', handleClickOutside);
    return () => document.removeEventListener('mousedown', handleClickOutside);
  }, [isOpen]);

  const handleAssign = async (tagId: string) => {
    try {
      await invoke('api_assign_meeting_tag', { meetingId, tagId });
      await loadTags();
      await refetchMeetings();
    } catch (error) {
      console.error('Failed to assign tag:', error);
      toast.error('Failed to add tag');
    }
  };

  const handleRemove = async (tagId: string) => {
    try {
      await invoke('api_remove_meeting_tag', { meetingId, tagId });
      setTags(prev => prev.filter(t => t.id !== tagId));
      await refetchMeetings();
    } catch (error) {
      console.error('Failed to remove tag:', error);
      toast.error('Failed to remove tag');
    }
  };

  const handleCreateAndAssign = async () => {
    const name = input.trim();
    if (!name) return;
    try {
      const tag = await invoke('api_create_tag', { name, color: null }) as MeetingTag;
      await handleAssign(tag.id);
      setInput('');
      setIsOpen(false);
    } catch (error) {
      console.error('Failed to create tag:', error);
      toast.error('Failed to create tag');
    }
  };

  if (isLoading || !meetingId || meetingId === 'intro-call') return null;

  const assignedIds = new Set(tags.map(t => t.id));
  const availableToAdd = allTags
    .filter(t => !assignedIds.has(t.id))
    .filter(t => t.name.toLowerCase().includes(input.toLowerCase()));

  return (
    <div className="flex items-center flex-wrap gap-1.5 px-4 py-2 border-b border-gray-100 bg-white">
      <TagIcon className="w-3.5 h-3.5 text-gray-400 flex-shrink-0" />
      {tags.map(tag => (
        <span
          key={tag.id}
          className="inline-flex items-center gap-1 px-2 py-0.5 text-xs rounded-full bg-blue-50 text-blue-700 border border-blue-200"
          style={tag.color ? { backgroundColor: tag.color, borderColor: tag.color, color: '#fff' } : undefined}
        >
          {tag.name}
          <button onClick={() => handleRemove(tag.id)} className="hover:opacity-70" aria-label={`Remove tag ${tag.name}`}>
            <X className="w-3 h-3" />
          </button>
        </span>
      ))}

      <div className="relative" ref={containerRef}>
        <button
          onClick={() => setIsOpen(o => !o)}
          className="inline-flex items-center gap-1 px-2 py-0.5 text-xs rounded-full border border-dashed border-gray-300 text-gray-500 hover:bg-gray-50"
        >
          <Plus className="w-3 h-3" />
          Add tag
        </button>
        {isOpen && (
          <div className="absolute z-10 mt-1 w-56 bg-white border border-gray-200 rounded-md shadow-lg p-2">
            <input
              autoFocus
              value={input}
              onChange={e => setInput(e.target.value)}
              onKeyDown={e => {
                if (e.key === 'Enter') handleCreateAndAssign();
                if (e.key === 'Escape') setIsOpen(false);
              }}
              placeholder="New or existing tag..."
              className="w-full text-xs border border-gray-200 rounded px-2 py-1 mb-1.5"
            />
            {availableToAdd.length > 0 && (
              <div className="max-h-40 overflow-y-auto space-y-0.5">
                {availableToAdd.map(t => (
                  <button
                    key={t.id}
                    onClick={() => { handleAssign(t.id); setIsOpen(false); setInput(''); }}
                    className="w-full text-left text-xs px-2 py-1 rounded hover:bg-gray-50"
                  >
                    {t.name}
                  </button>
                ))}
              </div>
            )}
            {input.trim() && !allTags.some(t => t.name.toLowerCase() === input.trim().toLowerCase()) && (
              <button
                onClick={handleCreateAndAssign}
                className="w-full text-left text-xs px-2 py-1 mt-0.5 rounded hover:bg-gray-50 text-blue-600"
              >
                Create "{input.trim()}"
              </button>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
