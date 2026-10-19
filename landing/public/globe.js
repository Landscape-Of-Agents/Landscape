"use strict";
// The Landscape of Agents signature: a revolving dotted globe. Agent nodes on its surface send calls
// to one another along arcs; most arrive and ring white (allowed), some stop short and ring red
// (denied, as Crane does). Satellites run along two orbit rings. Scrolling spins the globe faster,
// the pointer tilts it, and it dims below the hero. Under reduced motion it is one still frame.

(() => {
  const canvas = document.getElementById("globe");
  if (!canvas || !canvas.getContext) return;
  const context = canvas.getContext("2d");
  const still = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const small = window.matchMedia("(max-width: 640px)").matches;
  const GOLDEN = Math.PI * (3 - Math.sqrt(5));
  const COUNT = small ? 720 : 1300;
  const TILT = 0.36;
  const INK = "232, 232, 236";
  const DENY = "248, 113, 113";

  // Points spread evenly over the sphere (a Fibonacci lattice)
  const points = Array.from({ length: COUNT }, (_, index) => {
    const y = 1 - (2 * (index + 0.5)) / COUNT;
    const radius = Math.sqrt(1 - y * y);
    return [Math.cos(index * GOLDEN) * radius, y, Math.sin(index * GOLDEN) * radius];
  });
  // Agent nodes: a fixed handful of lattice points
  const nodes = Array.from({ length: small ? 12 : 18 }, (_, index) => points[Math.floor(((index * 0.6180339887 + 0.13) % 1) * COUNT)]);

  let width = 0, height = 0, ratio = 1, radius = 0;
  function resize() {
    ratio = Math.min(window.devicePixelRatio || 1, 1.75);
    const box = canvas.getBoundingClientRect();
    width = box.width;
    height = box.height;
    canvas.width = Math.round(width * ratio);
    canvas.height = Math.round(height * ratio);
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    radius = Math.min(width, height) * 0.36;
  }

  // Rotate a unit vector by the spin (around y) and the tilt (around x), then project it
  let spin = 0, tiltX = TILT, tiltY = 0;
  function project([x, y, z], lift = 1) {
    const cos = Math.cos(spin + tiltY), sin = Math.sin(spin + tiltY);
    const x1 = x * cos + z * sin, z1 = -x * sin + z * cos;
    const cosT = Math.cos(tiltX), sinT = Math.sin(tiltX);
    const y2 = y * cosT - z1 * sinT, z2 = y * sinT + z1 * cosT;
    const scale = (radius * lift) / (1 - z2 * 0.18);
    return { x: width / 2 + x1 * scale, y: height / 2 - y2 * scale, z: z2 };
  }

  // A point on the arc from a to b at t, raised above the surface in the middle
  function arcPoint(a, b, t, angle) {
    const sinAngle = Math.sin(angle) || 1e-6;
    const wa = Math.sin((1 - t) * angle) / sinAngle, wb = Math.sin(t * angle) / sinAngle;
    const point = [a[0] * wa + b[0] * wb, a[1] * wa + b[1] * wb, a[2] * wa + b[2] * wb];
    return project(point, 1 + Math.sin(Math.PI * t) * (0.12 + angle * 0.12));
  }

  // Calls between agents: travel, then ring where they end
  const calls = [];
  let nextCall = 0;
  function launch(now) {
    const from = nodes[Math.floor(Math.random() * nodes.length)];
    let to = from;
    while (to === from) to = nodes[Math.floor(Math.random() * nodes.length)];
    const angle = Math.acos(Math.max(-1, Math.min(1, from[0] * to[0] + from[1] * to[1] + from[2] * to[2])));
    if (angle < 0.35) return;
    const denied = Math.random() < 0.24;
    calls.push({ from, to, angle, start: now, travel: 1700 + angle * 700, stop: denied ? 0.62 + Math.random() * 0.18 : 1, denied });
  }

  function ring(point, age, color, size) {
    if (point.z < -0.15) return;
    const fade = 1 - age;
    context.beginPath();
    context.arc(point.x, point.y, 2 + age * size, 0, Math.PI * 2);
    context.strokeStyle = `rgba(${color}, ${0.55 * fade})`;
    context.lineWidth = 1.2;
    context.stroke();
  }

  function draw(now) {
    context.clearRect(0, 0, width, height);
    // Atmosphere: a soft rim of light around the sphere
    const glow = context.createRadialGradient(width / 2, height / 2, radius * 0.86, width / 2, height / 2, radius * 1.32);
    glow.addColorStop(0, `rgba(${INK}, 0.07)`);
    glow.addColorStop(1, `rgba(${INK}, 0)`);
    context.fillStyle = glow;
    context.fillRect(0, 0, width, height);

    // The dots: far side faint, near side brighter and larger
    for (const point of points) {
      const p = project(point);
      const near = (p.z + 1) / 2;
      const alpha = p.z < 0 ? 0.04 + near * 0.05 : 0.1 + near * near * 0.46;
      context.fillStyle = `rgba(${INK}, ${alpha})`;
      const size = (p.z < 0 ? 0.7 : 0.8 + near * 0.9);
      context.fillRect(p.x - size / 2, p.y - size / 2, size, size);
    }

    // Two orbit rings, each with satellites running along it
    for (const [index, orbit] of [[0, { lift: 1.2, incline: 0.5, speed: 0.00011 }], [1, { lift: 1.34, incline: -0.85, speed: -0.00007 }]]) {
      context.beginPath();
      const steps = 120;
      for (let step = 0; step <= steps; step += 1) {
        const angle = (step / steps) * Math.PI * 2;
        const p = project([Math.cos(angle), Math.sin(angle) * Math.sin(orbit.incline), Math.sin(angle) * Math.cos(orbit.incline)], orbit.lift);
        if (step) context.lineTo(p.x, p.y); else context.moveTo(p.x, p.y);
      }
      context.strokeStyle = `rgba(${INK}, ${index ? 0.05 : 0.07})`;
      context.lineWidth = 1;
      context.setLineDash([2, 6]);
      context.stroke();
      context.setLineDash([]);
      for (let satellite = 0; satellite < 2; satellite += 1) {
        const angle = now * orbit.speed + satellite * Math.PI;
        const p = project([Math.cos(angle), Math.sin(angle) * Math.sin(orbit.incline), Math.sin(angle) * Math.cos(orbit.incline)], orbit.lift);
        context.beginPath();
        context.arc(p.x, p.y, p.z < 0 ? 1.2 : 2.2, 0, Math.PI * 2);
        context.fillStyle = `rgba(${INK}, ${p.z < 0 ? 0.15 : 0.75})`;
        context.fill();
      }
    }

    // Agent nodes: a dot with a slow halo
    for (const [index, node] of nodes.entries()) {
      const p = project(node);
      if (p.z < -0.05) continue;
      const pulse = (Math.sin(now / 900 + index) + 1) / 2;
      context.beginPath();
      context.arc(p.x, p.y, 2.2, 0, Math.PI * 2);
      context.fillStyle = `rgba(${INK}, ${0.5 + p.z * 0.4})`;
      context.fill();
      context.beginPath();
      context.arc(p.x, p.y, 5 + pulse * 4, 0, Math.PI * 2);
      context.strokeStyle = `rgba(${INK}, ${0.08 + pulse * 0.08})`;
      context.stroke();
    }

    // Calls: a fading trail behind a bright head, then a ring where the call ended
    for (let index = calls.length - 1; index >= 0; index -= 1) {
      const call = calls[index];
      const elapsed = now - call.start;
      const progress = Math.min(call.stop, elapsed / call.travel);
      const after = (elapsed - call.travel * call.stop) / 900;
      if (after > 1) { calls.splice(index, 1); continue; }
      const tail = Math.max(0, progress - 0.32);
      const samples = 22;
      let previous = arcPoint(call.from, call.to, tail, call.angle);
      for (let step = 1; step <= samples; step += 1) {
        const t = tail + ((progress - tail) * step) / samples;
        const p = arcPoint(call.from, call.to, t, call.angle);
        if (p.z > -0.1 && previous.z > -0.1) {
          context.beginPath();
          context.moveTo(previous.x, previous.y);
          context.lineTo(p.x, p.y);
          const fade = after > 0 ? 1 - after : 1;
          context.strokeStyle = `rgba(${call.denied && after > 0 ? DENY : INK}, ${(step / samples) * 0.5 * fade})`;
          context.lineWidth = 1.3;
          context.stroke();
        }
        previous = p;
      }
      const head = arcPoint(call.from, call.to, progress, call.angle);
      if (after <= 0 && head.z > -0.1) {
        context.beginPath();
        context.arc(head.x, head.y, 2, 0, Math.PI * 2);
        context.fillStyle = `rgba(${INK}, 0.95)`;
        context.fill();
      }
      if (after > 0) ring(head, after, call.denied ? DENY : INK, call.denied ? 10 : 16);
    }
  }

  // Scrolling spins the globe faster and dims it below the hero; the pointer tilts it
  let lastScroll = window.scrollY, boost = 0, targetX = TILT, targetY = 0;
  window.addEventListener("scroll", () => {
    boost = Math.min(0.02, boost + Math.abs(window.scrollY - lastScroll) * 0.00004);
    lastScroll = window.scrollY;
    canvas.style.opacity = String(Math.max(0.35, 1 - window.scrollY / (window.innerHeight * 1.4)));
  }, { passive: true });
  window.addEventListener("pointermove", (event) => {
    targetY = (event.clientX / window.innerWidth - 0.5) * 0.35;
    targetX = TILT + (event.clientY / window.innerHeight - 0.5) * 0.2;
  }, { passive: true });
  window.addEventListener("resize", resize);
  resize();

  if (still) { spin = 0.6; draw(0); return; }
  let last = performance.now();
  function frame(now) {
    const delta = Math.min(64, now - last);
    last = now;
    if (!document.hidden) {
      spin += delta * (0.00007 + boost);
      boost *= 0.94;
      tiltX += (targetX - tiltX) * 0.04;
      tiltY += (targetY - tiltY) * 0.04;
      if (now > nextCall && calls.length < (small ? 4 : 7)) { launch(now); nextCall = now + 500 + Math.random() * 700; }
      draw(now);
    }
    requestAnimationFrame(frame);
  }
  requestAnimationFrame(frame);
})();
