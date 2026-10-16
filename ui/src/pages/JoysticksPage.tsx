import { useEffect, useState } from "react";
import { DndContext, PointerSensor, closestCenter, useSensor, useSensors } from "@dnd-kit/core";
import type { DragEndEvent } from "@dnd-kit/core";
import { SortableContext, arrayMove, useSortable, verticalListSortingStrategy } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { send } from "../backend";
import type { BackendState } from "../backend";
import type { JoystickSlot, VirtualJoystickState } from "../protocol";

const itemId = (slot: JoystickSlot) => slot.device?.id ?? `empty-${slot.slot}`;

function Row({ slot, index }: { slot: JoystickSlot; index: number }) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({
    id: itemId(slot),
  });
  return (
    <div
      ref={setNodeRef}
      className={`joystick-row ${isDragging ? "dragging" : ""}`}
      style={{ transform: CSS.Transform.toString(transform), transition }}
    >
      <span className="handle" {...attributes} {...listeners} aria-label="Drag to reorder">
        ⠿
      </span>
      <span className="flex-grow-1">
        {index}: {slot.device ? slot.device.name : <span className="text-body-secondary">Unbound</span>}
      </span>
      {slot.device && (
        <button
          type="button"
          className="btn btn-sm btn-outline-secondary py-0"
          title="Remove from this port"
          onClick={() => send({ type: "unbind_joystick", id: slot.device!.id })}
        >
          ×
        </button>
      )}
    </div>
  );
}

function VirtualState({ state }: { state: VirtualJoystickState }) {
  const names = ["Left X", "Left Y", "L Trigger", "R Trigger", "Right X", "Right Y"];
  const buttons = ["A", "B", "X", "Y", "LB", "RB", "Back", "Start", "LS", "RS"];
  return (
    <div className="small">
      {state.axes.map((value, i) => {
        const trigger = i === 2 || i === 3;
        const left = trigger ? 0 : Math.min(50, 50 + value * 50);
        const width = trigger ? value * 100 : Math.abs(value) * 50;
        return (
          <div key={i} className="d-flex align-items-center gap-2 mb-1">
            <span style={{ width: "5rem" }}>{names[i]}</span>
            <div className="axis-bar flex-grow-1">
              <span style={{ left: `${left}%`, width: `${width}%` }} />
            </div>
            <span style={{ width: "3rem" }} className="text-end">
              {value.toFixed(2)}
            </span>
          </div>
        );
      })}
      <div className="d-flex flex-wrap gap-1 mt-2">
        {buttons.map((b, i) => (
          <span key={b} className={`badge ${state.buttons[i] ? "bg-primary" : "bg-secondary"}`}>
            {b}
          </span>
        ))}
        <span className="badge bg-secondary">POV {state.pov < 0 ? "–" : `${state.pov}°`}</span>
      </div>
    </div>
  );
}

export function JoysticksPage({ state }: { state: BackendState }) {
  const s = state.snapshot!;
  // Show the dropped order until the backend confirms it
  const [order, setOrder] = useState<JoystickSlot[]>(s.joysticks);
  useEffect(() => setOrder(s.joysticks), [s.joysticks]);
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 4 } }));

  const onDragEnd = (ev: DragEndEvent) => {
    if (!ev.over || ev.active.id === ev.over.id) return;
    const from = order.findIndex((slot) => itemId(slot) === ev.active.id);
    const to = order.findIndex((slot) => itemId(slot) === ev.over!.id);
    setOrder(arrayMove(order, from, to));
    send({ type: "move_joystick", from, to });
  };

  const virtualBound = s.joysticks.some((j) => j.device?.virtual);

  return (
    <div className="container-fluid pt-3">
      <div className="row g-4">
        <div className="col-4">
          <h6>Ports</h6>
          <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={onDragEnd}>
            <SortableContext items={order.map(itemId)} strategy={verticalListSortingStrategy}>
              {order.map((slot, i) => (
                <Row key={itemId(slot)} slot={slot} index={i} />
              ))}
            </SortableContext>
          </DndContext>
        </div>
        <div className="col-4">
          <h6>Available devices</h6>
          {s.unbound_joysticks.length === 0 && <p className="small text-body-secondary">All devices are on a port.</p>}
          {s.unbound_joysticks.map((device) => (
            <div key={device.id} className="joystick-row">
              <span className="flex-grow-1">{device.name}</span>
              <button
                type="button"
                className="btn btn-sm btn-secondary py-0"
                onClick={() => send({ type: "bind_joystick", id: device.id, slot: null })}
              >
                Assign
              </button>
            </div>
          ))}
          <p className="small text-body-secondary mt-2">
            Drag ports to reorder. The virtual joystick lets AI agents drive over MCP and silences
            WPILib warnings about missing joysticks.
          </p>
        </div>
        <div className="col">
          {virtualBound && (
            <>
              <h6>Virtual joystick</h6>
              <VirtualState state={s.virtual_joystick} />
            </>
          )}
        </div>
      </div>
    </div>
  );
}
