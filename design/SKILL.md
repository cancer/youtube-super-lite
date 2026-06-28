---
name: youtube-super-lite-design
description: Use this skill to generate well-branded interfaces and assets for YouTube Super Lite, a dark, Japanese-first native video-player UI, either for production or throwaway prototypes/mocks/etc. Contains the primitive/semantic token system, colors, type, fonts, assets, and UI kit components for prototyping.
user-invocable: true
---

Read the README.md file within this skill, and explore the other available files (DESIGN.md in the source repo is the canonical spec; tokens/ holds the implemented primitive/semantic CSS).
If creating visual artifacts (slides, mocks, throwaway prototypes, etc), copy assets out and create static HTML files for the user to view. If working on production code, copy assets and follow the rules here to design with this system. Reference semantic tokens (--s-*) only; never hardcode primitive values.
If the user invokes this skill without any other guidance, ask them what they want to build or design, ask some questions, and act as an expert designer who outputs HTML artifacts _or_ production code, depending on the need.
